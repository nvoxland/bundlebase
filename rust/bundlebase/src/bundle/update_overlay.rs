//! Update overlay parquet I/O.
//!
//! Overlay files store updated cell values organized by block. Each row group
//! corresponds to one block and contains:
//! - `_row_number` (UInt32): the row number within the block
//! - Data columns named by ColumnId (hex string): the updated values
//! - `_updated_mask` (Binary): packed bitmask indicating which data columns were SET
//!
//! Column naming uses ColumnId (not user-visible name) to survive renames.
//! Row group metadata contains `block_ref` to identify which block the group belongs to.

use crate::object_id::ColumnId;
use arrow::array::{ArrayRef, BooleanArray};
use bundlebase_common::{BundlebaseError, RowId};
use bytes::Bytes;
use datafusion::scalar::ScalarValue;
use std::collections::HashMap;

/// Arrow-native overlay for a single block.
///
/// Row numbers are sorted ascending. Each column entry contains the values array
/// (aligned with row_numbers) and a boolean mask indicating which positions were
/// actually SET (to distinguish "SET x = NULL" from "x was not updated").
#[derive(Clone, Debug)]
pub struct UpdateOverlay {
    /// Row numbers within this block (sorted ascending). Length = N overlay rows.
    pub row_numbers: Vec<u32>,
    /// Column data for updated columns.
    /// Key: ColumnId, Value: (values array of length N, is_set boolean array of length N)
    pub columns: HashMap<ColumnId, (ArrayRef, BooleanArray)>,
}

impl UpdateOverlay {
    /// Build an UpdateOverlay from the pending updates HashMap (used for in-session flushing).
    pub fn from_pending(updates: &HashMap<RowId, HashMap<ColumnId, ScalarValue>>) -> Self {
        if updates.is_empty() {
            return Self {
                row_numbers: Vec::new(),
                columns: HashMap::new(),
            };
        }

        // Sort by row_number for binary search at scan time
        let mut rows: Vec<_> = updates.iter().collect();
        rows.sort_by_key(|(rid, _)| rid.row_number());

        let row_numbers: Vec<u32> = rows.iter().map(|(rid, _)| rid.row_number()).collect();

        // Collect all ColumnIds across all updates
        let mut all_col_ids: std::collections::HashSet<ColumnId> = std::collections::HashSet::new();
        for (_, cell_updates) in &rows {
            all_col_ids.extend(cell_updates.keys());
        }

        // Build per-column arrays
        let mut columns = HashMap::new();
        for col_id in all_col_ids {
            let mut values: Vec<ScalarValue> = Vec::with_capacity(rows.len());
            let mut is_set: Vec<bool> = Vec::with_capacity(rows.len());

            // Determine target type from first non-null value
            let target_type = rows
                .iter()
                .find_map(|(_, u)| u.get(&col_id))
                .map(|v| v.data_type())
                .unwrap_or(arrow::datatypes::DataType::Null);

            let typed_null = ScalarValue::try_from(&target_type).unwrap_or(ScalarValue::Null);

            for (_, cell_updates) in &rows {
                if let Some(val) = cell_updates.get(&col_id) {
                    values.push(val.clone());
                    is_set.push(true);
                } else {
                    values.push(typed_null.clone());
                    is_set.push(false);
                }
            }

            let array = ScalarValue::iter_to_array(values.into_iter())
                .unwrap_or_else(|_| arrow::array::new_empty_array(&target_type));
            let mask = BooleanArray::from(is_set);
            columns.insert(col_id, (array, mask));
        }

        Self {
            row_numbers,
            columns,
        }
    }

    /// Merge multiple overlays into one. Later overlays override earlier ones per-cell.
    ///
    /// Uses MutableArrayData to copy directly from source overlay arrays,
    /// avoiding Arrow->ScalarValue->Arrow round-trips.
    pub fn merge(overlays: &[Self]) -> Self {
        if overlays.is_empty() {
            return Self {
                row_numbers: Vec::new(),
                columns: HashMap::new(),
            };
        }
        if overlays.len() == 1 {
            return overlays[0].clone();
        }

        // Pass 1: determine which overlay wins for each (row_number, column_id).
        // Stores (overlay_idx, position_in_overlay) instead of ScalarValue.
        let mut winners: std::collections::BTreeMap<u32, HashMap<ColumnId, (usize, usize)>> =
            std::collections::BTreeMap::new();

        for (ov_idx, overlay) in overlays.iter().enumerate() {
            for (pos, &row_num) in overlay.row_numbers.iter().enumerate() {
                let entry = winners.entry(row_num).or_default();
                for (col_id, (_values, is_set)) in &overlay.columns {
                    if is_set.value(pos) {
                        entry.insert(*col_id, (ov_idx, pos));
                    }
                }
            }
        }

        let row_numbers: Vec<u32> = winners.keys().copied().collect();
        let n = row_numbers.len();

        // Collect all column IDs across overlays
        let mut all_col_ids: std::collections::HashSet<ColumnId> = std::collections::HashSet::new();
        for overlay in overlays {
            all_col_ids.extend(overlay.columns.keys());
        }

        // Pass 2: build output arrays via MutableArrayData
        let mut columns = HashMap::new();
        for col_id in all_col_ids {
            // Determine data type from first overlay that has this column
            let data_type = overlays
                .iter()
                .find_map(|ov| {
                    ov.columns
                        .get(&col_id)
                        .map(|(arr, _)| arr.data_type().clone())
                })
                .unwrap_or(arrow::datatypes::DataType::Null);

            // Build source array data for MutableArrayData — one per overlay.
            // Use a 1-element null filler for overlays that lack this column.
            let null_filler = arrow::array::new_null_array(&data_type, 1);
            let null_filler_data = null_filler.to_data();

            let source_data: Vec<arrow::array::ArrayData> = overlays
                .iter()
                .map(|ov| match ov.columns.get(&col_id) {
                    Some((arr, _)) => arr.to_data(),
                    None => null_filler_data.clone(),
                })
                .collect();
            let source_refs: Vec<&arrow::array::ArrayData> = source_data.iter().collect();

            let mut builder = arrow::array::MutableArrayData::new(source_refs, true, n);

            let mut is_set_vec: Vec<bool> = Vec::with_capacity(n);

            for &row_num in &row_numbers {
                if let Some(&(ov_idx, src_pos)) = winners.get(&row_num).and_then(|m| m.get(&col_id))
                {
                    builder.extend(ov_idx, src_pos, src_pos + 1);
                    is_set_vec.push(true);
                } else {
                    builder.extend_nulls(1);
                    is_set_vec.push(false);
                }
            }

            let array = arrow::array::make_array(builder.freeze());
            let mask = BooleanArray::from(is_set_vec);
            columns.insert(col_id, (array, mask));
        }

        Self {
            row_numbers,
            columns,
        }
    }
}

const ROW_NUMBER_COL: &str = "_row_number";
const MASK_COL: &str = "_updated_mask";
const BLOCK_REF_KEY: &str = "block_ref";

/// Write overlay data to parquet bytes, one row group per block.
///
/// `pending` maps RowId → (ColumnId → ScalarValue) for all updates in the transaction.
/// `column_types` maps ColumnId → Arrow DataType for schema construction.
pub fn write_overlay_parquet(
    pending: &HashMap<RowId, HashMap<ColumnId, ScalarValue>>,
    column_types: &HashMap<ColumnId, arrow::datatypes::DataType>,
) -> Result<Bytes, BundlebaseError> {
    use arrow::array::*;
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;

    if pending.is_empty() {
        return Err("No updates to write".into());
    }

    // Group by block_ref
    let mut by_block: std::collections::BTreeMap<
        u16,
        Vec<(&RowId, &HashMap<ColumnId, ScalarValue>)>,
    > = std::collections::BTreeMap::new();
    for (row_id, cell_updates) in pending {
        by_block
            .entry(row_id.block_ref().as_u16())
            .or_default()
            .push((row_id, cell_updates));
    }

    // Sort rows within each block by row_number
    for rows in by_block.values_mut() {
        rows.sort_by_key(|(rid, _)| rid.row_number());
    }

    // Collect all ColumnIds, sorted for deterministic ordering
    let mut all_col_ids: Vec<ColumnId> = column_types.keys().cloned().collect();
    all_col_ids.sort_by_key(|id| id.to_string());

    // Build column index map for bitmask
    let col_index: HashMap<ColumnId, usize> = all_col_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();

    // Build schema: _row_number + data columns + _updated_mask
    let mut fields = vec![Field::new(ROW_NUMBER_COL, DataType::UInt32, false)];
    for col_id in &all_col_ids {
        let dt = column_types
            .get(col_id)
            .ok_or_else(|| BundlebaseError::from(format!("Missing type for column {}", col_id)))?;
        fields.push(Field::new(col_id.to_string(), dt.clone(), true));
    }
    fields.push(Field::new(MASK_COL, DataType::Binary, false));
    let schema = std::sync::Arc::new(Schema::new(fields));

    // Write one row group per block
    let mut buffer = Vec::new();
    {
        let props = parquet::file::properties::WriterProperties::builder().build();
        let mut writer =
            ArrowWriter::try_new(&mut buffer, schema.clone(), Some(props)).map_err(|e| {
                BundlebaseError::from(format!("Failed to create parquet writer: {}", e))
            })?;

        for (block_ref, rows) in &by_block {
            let num_rows = rows.len();

            // Build _row_number array
            let row_number_values: Vec<u32> =
                rows.iter().map(|(rid, _)| rid.row_number()).collect();
            let row_number_array: ArrayRef =
                std::sync::Arc::new(UInt32Array::from(row_number_values));

            // Build data column arrays
            let mask_bytes_len = (all_col_ids.len() + 7) / 8;
            let mut data_arrays: Vec<ArrayRef> = Vec::new();
            for col_id in &all_col_ids {
                let dt = &column_types[col_id];
                let typed_null = ScalarValue::try_from(dt).map_err(|e| {
                    BundlebaseError::from(format!("Cannot create null for type {:?}: {}", dt, e))
                })?;
                let scalars: Vec<ScalarValue> = rows
                    .iter()
                    .map(|(_, updates)| match updates.get(col_id) {
                        Some(val) if val.is_null() => Ok(typed_null.clone()),
                        Some(val) if val.data_type() != *dt => val.cast_to(dt).map_err(|e| {
                            BundlebaseError::from(format!(
                                "Failed to cast column {} from {:?} to {:?}: {}",
                                col_id,
                                val.data_type(),
                                dt,
                                e
                            ))
                        }),
                        Some(val) => Ok(val.clone()),
                        None => Ok(typed_null.clone()),
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let array = if scalars.is_empty() {
                    arrow::array::new_empty_array(dt)
                } else {
                    ScalarValue::iter_to_array(scalars.into_iter()).map_err(|e| {
                        BundlebaseError::from(format!(
                            "Failed to build array for column {}: {}",
                            col_id, e
                        ))
                    })?
                };
                data_arrays.push(array);
            }

            // Build _updated_mask array
            let mut mask_values: Vec<Vec<u8>> = Vec::with_capacity(num_rows);
            for (_, updates) in rows {
                let mut mask = vec![0u8; mask_bytes_len];
                for (col_id, _) in updates.iter() {
                    if let Some(&idx) = col_index.get(col_id) {
                        mask[idx / 8] |= 1 << (idx % 8);
                    }
                }
                mask_values.push(mask);
            }
            let mask_array: ArrayRef = std::sync::Arc::new(BinaryArray::from(
                mask_values
                    .iter()
                    .map(|v| v.as_slice())
                    .collect::<Vec<&[u8]>>(),
            ));

            // Assemble columns
            let mut columns = vec![row_number_array];
            columns.extend(data_arrays);
            columns.push(mask_array);

            let batch = RecordBatch::try_new(schema.clone(), columns).map_err(|e| {
                BundlebaseError::from(format!("Failed to build overlay batch: {}", e))
            })?;

            // Write row group with block_ref metadata
            writer.append_key_value_metadata(parquet::file::metadata::KeyValue::new(
                BLOCK_REF_KEY.to_string(),
                block_ref.to_string(),
            ));

            writer.write(&batch).map_err(|e| {
                BundlebaseError::from(format!("Failed to write overlay parquet: {}", e))
            })?;
            writer
                .flush()
                .map_err(|e| BundlebaseError::from(format!("Failed to flush row group: {}", e)))?;
        }

        writer.close().map_err(|e| {
            BundlebaseError::from(format!("Failed to close overlay parquet: {}", e))
        })?;
    }

    Ok(Bytes::from(buffer))
}

/// Read overlay parquet bytes into per-block UpdateOverlays.
///
/// Returns a vec of (block_ref, UpdateOverlay) pairs. Each overlay contains
/// Arrow arrays directly from the parquet row group — no ScalarValue extraction.
pub fn read_overlay_parquet(bytes: &[u8]) -> Result<Vec<(u16, UpdateOverlay)>, BundlebaseError> {
    use arrow::array::*;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    let parquet_bytes = Bytes::copy_from_slice(bytes);

    // Read file metadata to get row group count and block_ref values
    let builder = ParquetRecordBatchReaderBuilder::try_new(parquet_bytes.clone())
        .map_err(|e| BundlebaseError::from(format!("Failed to open overlay parquet: {}", e)))?;

    let file_metadata = builder.metadata().file_metadata().clone();
    let num_row_groups = builder.metadata().num_row_groups();
    let schema = builder.schema().clone();

    // Extract block_ref values from file-level key-value metadata
    let block_refs: Vec<u16> = file_metadata
        .key_value_metadata()
        .map(|kvs| {
            kvs.iter()
                .filter(|kv| kv.key == BLOCK_REF_KEY)
                .filter_map(|kv| kv.value.as_ref()?.parse::<u16>().ok())
                .collect()
        })
        .unwrap_or_default();

    // Identify data columns (not _row_number or _updated_mask)
    let data_col_ids: Vec<(usize, ColumnId)> = schema
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name() != ROW_NUMBER_COL && f.name() != MASK_COL)
        .map(|(i, f)| {
            let col_id = ColumnId::try_from(f.name().as_str()).map_err(|e| {
                BundlebaseError::from(format!("Invalid ColumnId '{}': {}", f.name(), e))
            });
            (i, col_id)
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|(i, r)| Ok((i, r?)))
        .collect::<Result<Vec<_>, BundlebaseError>>()?;

    let row_number_idx = schema
        .index_of(ROW_NUMBER_COL)
        .map_err(|e| BundlebaseError::from(format!("Missing _row_number column: {}", e)))?;
    let mask_idx = schema
        .index_of(MASK_COL)
        .map_err(|e| BundlebaseError::from(format!("Missing _updated_mask column: {}", e)))?;

    let mut results = Vec::with_capacity(num_row_groups);

    // Read each row group separately
    for rg_idx in 0..num_row_groups {
        let block_ref = block_refs.get(rg_idx).copied().ok_or_else(|| {
            BundlebaseError::from(format!(
                "Missing block_ref metadata for row group {}",
                rg_idx
            ))
        })?;

        let reader = ParquetRecordBatchReaderBuilder::try_new(parquet_bytes.clone())
            .map_err(|e| BundlebaseError::from(format!("Failed to open overlay parquet: {}", e)))?
            .with_row_groups(vec![rg_idx])
            .build()
            .map_err(|e| BundlebaseError::from(format!("Failed to build parquet reader: {}", e)))?;

        // Collect all batches for this row group
        let mut all_row_numbers: Vec<u32> = Vec::new();
        let mut all_columns: HashMap<ColumnId, (Vec<ArrayRef>, Vec<BooleanArray>)> = HashMap::new();

        for batch_result in reader {
            let batch = batch_result.map_err(|e| {
                BundlebaseError::from(format!("Failed to read overlay batch: {}", e))
            })?;

            let row_number_col = batch
                .column(row_number_idx)
                .as_any()
                .downcast_ref::<UInt32Array>()
                .ok_or_else(|| BundlebaseError::from("_row_number column is not UInt32"))?;

            let mask_col = batch
                .column(mask_idx)
                .as_any()
                .downcast_ref::<BinaryArray>()
                .ok_or_else(|| BundlebaseError::from("_updated_mask column is not Binary"))?;

            // Extract row numbers
            for i in 0..row_number_col.len() {
                all_row_numbers.push(row_number_col.value(i));
            }

            // For each data column, extract the values and build is_set mask from bitmask
            for (data_idx, (schema_idx, col_id)) in data_col_ids.iter().enumerate() {
                let values_array = batch.column(*schema_idx).clone();

                // Decode bitmask into BooleanArray for this column
                let mut is_set_vec = Vec::with_capacity(batch.num_rows());
                for row in 0..batch.num_rows() {
                    let mask_bytes = mask_col.value(row);
                    let byte_idx = data_idx / 8;
                    let bit_idx = data_idx % 8;
                    let set =
                        byte_idx < mask_bytes.len() && (mask_bytes[byte_idx] & (1 << bit_idx)) != 0;
                    is_set_vec.push(set);
                }
                let is_set = BooleanArray::from(is_set_vec);

                let entry = all_columns
                    .entry(*col_id)
                    .or_insert_with(|| (Vec::new(), Vec::new()));
                entry.0.push(values_array);
                entry.1.push(is_set);
            }
        }

        // Concatenate batch chunks into single arrays per column
        let mut columns: HashMap<ColumnId, (ArrayRef, BooleanArray)> = HashMap::new();
        for (col_id, (value_chunks, mask_chunks)) in all_columns {
            let values = if value_chunks.len() == 1 {
                value_chunks
                    .into_iter()
                    .next()
                    .ok_or_else(|| BundlebaseError::from("Expected single value chunk"))?
            } else {
                let refs: Vec<&dyn arrow::array::Array> =
                    value_chunks.iter().map(|a| a.as_ref()).collect();
                arrow::compute::concat(&refs).map_err(|e| {
                    BundlebaseError::from(format!("Failed to concat overlay arrays: {}", e))
                })?
            };
            let mask = if mask_chunks.len() == 1 {
                mask_chunks
                    .into_iter()
                    .next()
                    .ok_or_else(|| BundlebaseError::from("Expected single mask chunk"))?
            } else {
                let refs: Vec<&dyn arrow::array::Array> = mask_chunks
                    .iter()
                    .map(|a| a as &dyn arrow::array::Array)
                    .collect();
                let combined = arrow::compute::concat(&refs).map_err(|e| {
                    BundlebaseError::from(format!("Failed to concat mask arrays: {}", e))
                })?;
                combined
                    .as_any()
                    .downcast_ref::<BooleanArray>()
                    .ok_or_else(|| BundlebaseError::from("Failed to downcast concatenated mask"))?
                    .clone()
            };
            columns.insert(col_id, (values, mask));
        }

        results.push((
            block_ref,
            UpdateOverlay {
                row_numbers: all_row_numbers,
                columns,
            },
        ));
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::ArrayRef;
    use arrow::datatypes::DataType;
    use bundlebase_common::ObjectIdAlias;
    use std::sync::Arc;

    #[test]
    fn test_roundtrip_single_column() {
        let col_id = ColumnId::generate();
        let row_id = RowId::new(ObjectIdAlias::from(0u16), 42);

        let mut pending = HashMap::new();
        let mut cell = HashMap::new();
        cell.insert(col_id, ScalarValue::Int64(Some(100)));
        pending.insert(row_id, cell);

        let mut column_types = HashMap::new();
        column_types.insert(col_id, DataType::Int64);

        let bytes = write_overlay_parquet(&pending, &column_types).expect("write failed");
        let overlays = read_overlay_parquet(&bytes).expect("read failed");

        assert_eq!(overlays.len(), 1);
        let (block_ref, overlay) = &overlays[0];
        assert_eq!(*block_ref, 0);
        assert_eq!(overlay.row_numbers, vec![42]);
        let (values, is_set) = overlay.columns.get(&col_id).expect("column not found");
        assert_eq!(values.len(), 1);
        assert!(is_set.value(0));
        assert_eq!(
            ScalarValue::try_from_array(values, 0).expect("read value"),
            ScalarValue::Int64(Some(100))
        );
    }

    #[test]
    fn test_roundtrip_null_value() {
        let col_id = ColumnId::generate();
        let row_id = RowId::new(ObjectIdAlias::from(0u16), 10);

        let mut pending = HashMap::new();
        let mut cell = HashMap::new();
        cell.insert(col_id, ScalarValue::Utf8(None)); // SET TO NULL
        pending.insert(row_id, cell);

        let mut column_types = HashMap::new();
        column_types.insert(col_id, DataType::Utf8);

        let bytes = write_overlay_parquet(&pending, &column_types).expect("write failed");
        let overlays = read_overlay_parquet(&bytes).expect("read failed");

        let (_, overlay) = &overlays[0];
        let (values, is_set) = overlay.columns.get(&col_id).expect("column not found");
        assert!(is_set.value(0));
        assert_eq!(
            ScalarValue::try_from_array(values, 0).expect("read value"),
            ScalarValue::Utf8(None)
        );
    }

    #[test]
    fn test_roundtrip_partial_update() {
        let col_a = ColumnId::generate();
        let col_b = ColumnId::generate();
        let row_id = RowId::new(ObjectIdAlias::from(0u16), 5);

        // Only update col_a, not col_b
        let mut pending = HashMap::new();
        let mut cell = HashMap::new();
        cell.insert(col_a, ScalarValue::Float64(Some(3.14)));
        pending.insert(row_id, cell);

        let mut column_types = HashMap::new();
        column_types.insert(col_a, DataType::Float64);
        column_types.insert(col_b, DataType::Utf8);

        let bytes = write_overlay_parquet(&pending, &column_types).expect("write failed");
        let overlays = read_overlay_parquet(&bytes).expect("read failed");

        let (_, overlay) = &overlays[0];
        // col_a should be set
        let (_, is_set_a) = overlay.columns.get(&col_a).expect("col_a not found");
        assert!(is_set_a.value(0));
        // col_b should not be set
        let (_, is_set_b) = overlay.columns.get(&col_b).expect("col_b not found");
        assert!(!is_set_b.value(0));
    }

    #[test]
    fn test_multiple_blocks() {
        let col_id = ColumnId::generate();
        let row_a = RowId::new(ObjectIdAlias::from(0u16), 1);
        let row_b = RowId::new(ObjectIdAlias::from(1u16), 2);

        let mut pending = HashMap::new();
        let mut cell_a = HashMap::new();
        cell_a.insert(col_id, ScalarValue::Int64(Some(10)));
        pending.insert(row_a, cell_a);
        let mut cell_b = HashMap::new();
        cell_b.insert(col_id, ScalarValue::Int64(Some(20)));
        pending.insert(row_b, cell_b);

        let mut column_types = HashMap::new();
        column_types.insert(col_id, DataType::Int64);

        let bytes = write_overlay_parquet(&pending, &column_types).expect("write failed");
        let overlays = read_overlay_parquet(&bytes).expect("read failed");

        assert_eq!(overlays.len(), 2);
        // Block 0
        let (ref0, ov0) = &overlays[0];
        assert_eq!(*ref0, 0);
        assert_eq!(ov0.row_numbers, vec![1]);
        // Block 1
        let (ref1, ov1) = &overlays[1];
        assert_eq!(*ref1, 1);
        assert_eq!(ov1.row_numbers, vec![2]);
    }

    #[test]
    fn test_from_pending() {
        let col_id = ColumnId::generate();
        let row_id = RowId::new(ObjectIdAlias::from(0u16), 5);

        let mut updates = HashMap::new();
        let mut cell = HashMap::new();
        cell.insert(col_id, ScalarValue::Int64(Some(42)));
        updates.insert(row_id, cell);

        let overlay = UpdateOverlay::from_pending(&updates);
        assert_eq!(overlay.row_numbers, vec![5]);
        let (values, is_set) = overlay.columns.get(&col_id).expect("column not found");
        assert!(is_set.value(0));
        assert_eq!(
            ScalarValue::try_from_array(values, 0).expect("read value"),
            ScalarValue::Int64(Some(42))
        );
    }

    #[test]
    fn test_merge_overlapping_same_column() {
        // Two overlays update the same row+column; later overlay wins
        let col_id = ColumnId::generate();

        let ov1 = UpdateOverlay {
            row_numbers: vec![5, 10],
            columns: {
                let values: ArrayRef = Arc::new(arrow::array::Int64Array::from(vec![100, 200]));
                let is_set = BooleanArray::from(vec![true, true]);
                let mut m = HashMap::new();
                m.insert(col_id, (values, is_set));
                m
            },
        };
        let ov2 = UpdateOverlay {
            row_numbers: vec![5],
            columns: {
                let values: ArrayRef = Arc::new(arrow::array::Int64Array::from(vec![999]));
                let is_set = BooleanArray::from(vec![true]);
                let mut m = HashMap::new();
                m.insert(col_id, (values, is_set));
                m
            },
        };

        let merged = UpdateOverlay::merge(&[ov1, ov2]);
        assert_eq!(merged.row_numbers, vec![5, 10]);

        let (values, is_set) = merged.columns.get(&col_id).expect("column missing");
        assert!(is_set.value(0));
        assert!(is_set.value(1));
        // Row 5: ov2 wins with 999
        assert_eq!(
            ScalarValue::try_from_array(values, 0).expect("read"),
            ScalarValue::Int64(Some(999))
        );
        // Row 10: only ov1 had it, stays 200
        assert_eq!(
            ScalarValue::try_from_array(values, 1).expect("read"),
            ScalarValue::Int64(Some(200))
        );
    }

    #[test]
    fn test_merge_different_columns() {
        // Two overlays update different columns on the same row
        let col_a = ColumnId::generate();
        let col_b = ColumnId::generate();

        let ov1 = UpdateOverlay {
            row_numbers: vec![3],
            columns: {
                let mut m = HashMap::new();
                m.insert(
                    col_a,
                    (
                        Arc::new(arrow::array::Int64Array::from(vec![10])) as ArrayRef,
                        BooleanArray::from(vec![true]),
                    ),
                );
                m
            },
        };
        let ov2 = UpdateOverlay {
            row_numbers: vec![3],
            columns: {
                let mut m = HashMap::new();
                m.insert(
                    col_b,
                    (
                        Arc::new(arrow::array::StringArray::from(vec!["hello"])) as ArrayRef,
                        BooleanArray::from(vec![true]),
                    ),
                );
                m
            },
        };

        let merged = UpdateOverlay::merge(&[ov1, ov2]);
        assert_eq!(merged.row_numbers, vec![3]);

        let (val_a, set_a) = merged.columns.get(&col_a).expect("col_a missing");
        assert!(set_a.value(0));
        assert_eq!(
            ScalarValue::try_from_array(val_a, 0).expect("read"),
            ScalarValue::Int64(Some(10))
        );

        let (val_b, set_b) = merged.columns.get(&col_b).expect("col_b missing");
        assert!(set_b.value(0));
        assert_eq!(
            ScalarValue::try_from_array(val_b, 0).expect("read"),
            ScalarValue::Utf8(Some("hello".to_string()))
        );
    }

    #[test]
    fn test_merge_disjoint_rows() {
        let col_id = ColumnId::generate();

        let ov1 = UpdateOverlay {
            row_numbers: vec![1, 3],
            columns: {
                let mut m = HashMap::new();
                m.insert(
                    col_id,
                    (
                        Arc::new(arrow::array::Int64Array::from(vec![10, 30])) as ArrayRef,
                        BooleanArray::from(vec![true, true]),
                    ),
                );
                m
            },
        };
        let ov2 = UpdateOverlay {
            row_numbers: vec![2, 4],
            columns: {
                let mut m = HashMap::new();
                m.insert(
                    col_id,
                    (
                        Arc::new(arrow::array::Int64Array::from(vec![20, 40])) as ArrayRef,
                        BooleanArray::from(vec![true, true]),
                    ),
                );
                m
            },
        };

        let merged = UpdateOverlay::merge(&[ov1, ov2]);
        assert_eq!(merged.row_numbers, vec![1, 2, 3, 4]);

        let (values, is_set) = merged.columns.get(&col_id).expect("column missing");
        for i in 0..4 {
            assert!(is_set.value(i));
        }
        assert_eq!(
            ScalarValue::try_from_array(values, 0).expect("r"),
            ScalarValue::Int64(Some(10))
        );
        assert_eq!(
            ScalarValue::try_from_array(values, 1).expect("r"),
            ScalarValue::Int64(Some(20))
        );
        assert_eq!(
            ScalarValue::try_from_array(values, 2).expect("r"),
            ScalarValue::Int64(Some(30))
        );
        assert_eq!(
            ScalarValue::try_from_array(values, 3).expect("r"),
            ScalarValue::Int64(Some(40))
        );
    }
}
