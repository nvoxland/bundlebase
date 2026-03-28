//! Update overlay parquet I/O.
//!
//! Overlay files store updated cell values keyed by RowId. Each row has:
//! - `_rowid` (UInt64): the RowId identifying the updated row
//! - Data columns named by ColumnId (hex string): the updated values
//! - `_updated_mask` (Binary): packed bitmask indicating which data columns were SET
//!
//! Column naming uses ColumnId (not user-visible name) to survive renames.

use bundlebase_common::{BundlebaseError, RowId};
use crate::object_id::ColumnId;
use bytes::Bytes;
use datafusion::scalar::ScalarValue;
use std::collections::HashMap;

/// Decoded overlay: RowId → (ColumnId → value).
/// Only contains columns that were actually SET (bitmask decoded at load time).
#[derive(Clone, Debug)]
pub struct UpdateOverlay {
    pub updates: HashMap<RowId, HashMap<ColumnId, ScalarValue>>,
}

const ROWID_COL: &str = "_rowid";
const MASK_COL: &str = "_updated_mask";

/// Write overlay data to parquet bytes.
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

    // Collect all ColumnIds that appear in any update, sorted for deterministic ordering
    let mut all_col_ids: Vec<ColumnId> = column_types.keys().cloned().collect();
    all_col_ids.sort_by_key(|id| id.to_string());

    // Build column index map for bitmask
    let col_index: HashMap<ColumnId, usize> = all_col_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();

    // Build schema: _rowid + data columns + _updated_mask
    let mut fields = vec![Field::new(ROWID_COL, DataType::UInt64, false)];
    for col_id in &all_col_ids {
        let dt = column_types.get(col_id)
            .ok_or_else(|| BundlebaseError::from(format!("Missing type for column {}", col_id)))?;
        fields.push(Field::new(col_id.to_string(), dt.clone(), true));
    }
    fields.push(Field::new(MASK_COL, DataType::Binary, false));
    let schema = std::sync::Arc::new(Schema::new(fields));

    // Sort rows by RowId for deterministic output
    let mut rows: Vec<_> = pending.iter().collect();
    rows.sort_by_key(|(rid, _)| rid.as_u64());

    let num_rows = rows.len();

    // Build _rowid array
    let rowid_values: Vec<u64> = rows.iter().map(|(rid, _)| rid.as_u64()).collect();
    let rowid_array: ArrayRef = std::sync::Arc::new(UInt64Array::from(rowid_values));

    // Build data column arrays
    let mut data_arrays: Vec<ArrayRef> = Vec::new();
    for col_id in &all_col_ids {
        let dt = &column_types[col_id];
        // Build array of ScalarValues for this column
        let typed_null = ScalarValue::try_from(dt)
            .map_err(|e| BundlebaseError::from(format!("Cannot create null for type {:?}: {}", dt, e)))?;
        let scalars: Vec<ScalarValue> = rows.iter().map(|(_, updates)| {
            match updates.get(col_id) {
                Some(val) if val.is_null() => Ok(typed_null.clone()),
                Some(val) if val.data_type() != *dt => {
                    val.cast_to(dt)
                        .map_err(|e| BundlebaseError::from(format!(
                            "Failed to cast column {} from {:?} to {:?}: {}",
                            col_id, val.data_type(), dt, e
                        )))
                }
                Some(val) => Ok(val.clone()),
                None => Ok(typed_null.clone()),
            }
        }).collect::<Result<Vec<_>, _>>()?;

        let array = if scalars.is_empty() {
            arrow::array::new_empty_array(dt)
        } else {
            ScalarValue::iter_to_array(scalars.into_iter())
                .map_err(|e| BundlebaseError::from(format!("Failed to build array for column {}: {}", col_id, e)))?
        };
        data_arrays.push(array);
    }

    // Build _updated_mask array
    let mask_bytes_len = (all_col_ids.len() + 7) / 8;
    let mut mask_values: Vec<Vec<u8>> = Vec::with_capacity(num_rows);
    for (_, updates) in &rows {
        let mut mask = vec![0u8; mask_bytes_len];
        for (col_id, _) in updates.iter() {
            if let Some(&idx) = col_index.get(col_id) {
                mask[idx / 8] |= 1 << (idx % 8);
            }
        }
        mask_values.push(mask);
    }
    let mask_array: ArrayRef = std::sync::Arc::new(
        BinaryArray::from(mask_values.iter().map(|v| v.as_slice()).collect::<Vec<&[u8]>>())
    );

    // Assemble columns
    let mut columns = vec![rowid_array];
    columns.extend(data_arrays);
    columns.push(mask_array);

    let batch = RecordBatch::try_new(schema.clone(), columns)
        .map_err(|e| BundlebaseError::from(format!("Failed to build overlay batch: {}", e)))?;

    // Write to parquet
    let mut buffer = Vec::new();
    {
        let props = parquet::file::properties::WriterProperties::builder().build();
        let mut writer = ArrowWriter::try_new(&mut buffer, schema, Some(props))
            .map_err(|e| BundlebaseError::from(format!("Failed to create parquet writer: {}", e)))?;
        writer.write(&batch)
            .map_err(|e| BundlebaseError::from(format!("Failed to write overlay parquet: {}", e)))?;
        writer.close()
            .map_err(|e| BundlebaseError::from(format!("Failed to close overlay parquet: {}", e)))?;
    }

    Ok(Bytes::from(buffer))
}

/// Read overlay parquet bytes into an UpdateOverlay.
pub fn read_overlay_parquet(bytes: &[u8]) -> Result<UpdateOverlay, BundlebaseError> {
    use arrow::array::*;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    let reader = ParquetRecordBatchReaderBuilder::try_new(Bytes::copy_from_slice(bytes))
        .map_err(|e| BundlebaseError::from(format!("Failed to open overlay parquet: {}", e)))?
        .build()
        .map_err(|e| BundlebaseError::from(format!("Failed to build parquet reader: {}", e)))?;

    let schema = reader.schema();

    // Identify data columns (not _rowid or _updated_mask)
    let data_col_ids: Vec<(usize, ColumnId)> = schema.fields().iter().enumerate()
        .filter(|(_, f)| f.name() != ROWID_COL && f.name() != MASK_COL)
        .map(|(i, f)| {
            let col_id = ColumnId::try_from(f.name().as_str())
                .map_err(|e| BundlebaseError::from(format!("Invalid ColumnId '{}': {}", f.name(), e)));
            (i, col_id)
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|(i, r)| Ok((i, r?)))
        .collect::<Result<Vec<_>, BundlebaseError>>()?;

    let rowid_idx = schema.index_of(ROWID_COL)
        .map_err(|e| BundlebaseError::from(format!("Missing _rowid column: {}", e)))?;
    let mask_idx = schema.index_of(MASK_COL)
        .map_err(|e| BundlebaseError::from(format!("Missing _updated_mask column: {}", e)))?;

    let mut updates: HashMap<RowId, HashMap<ColumnId, ScalarValue>> = HashMap::new();

    for batch_result in reader {
        let batch = batch_result
            .map_err(|e| BundlebaseError::from(format!("Failed to read overlay batch: {}", e)))?;

        let rowid_col = batch.column(rowid_idx)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .ok_or_else(|| BundlebaseError::from("_rowid column is not UInt64"))?;

        let mask_col = batch.column(mask_idx)
            .as_any()
            .downcast_ref::<BinaryArray>()
            .ok_or_else(|| BundlebaseError::from("_updated_mask column is not Binary"))?;

        for row in 0..batch.num_rows() {
            let row_id = RowId::from(rowid_col.value(row));
            let mask_bytes = mask_col.value(row);

            let mut cell_updates = HashMap::new();
            for (schema_idx, col_id) in &data_col_ids {
                // Check if this column was updated (bit set in mask)
                let bit_pos = data_col_ids.iter().position(|(_, id)| id == col_id)
                    .ok_or_else(|| BundlebaseError::from("Column not found in data_col_ids"))?;
                let byte_idx = bit_pos / 8;
                let bit_idx = bit_pos % 8;

                if byte_idx < mask_bytes.len() && (mask_bytes[byte_idx] & (1 << bit_idx)) != 0 {
                    let value = ScalarValue::try_from_array(batch.column(*schema_idx), row)
                        .map_err(|e| BundlebaseError::from(format!("Failed to read value: {}", e)))?;
                    cell_updates.insert(*col_id, value);
                }
            }

            if !cell_updates.is_empty() {
                updates.insert(row_id, cell_updates);
            }
        }
    }

    Ok(UpdateOverlay { updates })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::DataType;
    use bundlebase_common::ObjectIdAlias;

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
        let overlay = read_overlay_parquet(&bytes).expect("read failed");

        assert_eq!(overlay.updates.len(), 1);
        let row = overlay.updates.get(&row_id).expect("row not found");
        assert_eq!(row.get(&col_id), Some(&ScalarValue::Int64(Some(100))));
    }

    #[test]
    fn test_roundtrip_null_value() {
        let col_id = ColumnId::generate();
        let row_id = RowId::new(ObjectIdAlias::from(0u16), 10);

        let mut pending = HashMap::new();
        let mut cell = HashMap::new();
        cell.insert(col_id, ScalarValue::Utf8(None));  // SET TO NULL
        pending.insert(row_id, cell);

        let mut column_types = HashMap::new();
        column_types.insert(col_id, DataType::Utf8);

        let bytes = write_overlay_parquet(&pending, &column_types).expect("write failed");
        let overlay = read_overlay_parquet(&bytes).expect("read failed");

        let row = overlay.updates.get(&row_id).expect("row not found");
        assert_eq!(row.get(&col_id), Some(&ScalarValue::Utf8(None)));
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
        // col_b intentionally NOT in this update
        pending.insert(row_id, cell);

        let mut column_types = HashMap::new();
        column_types.insert(col_a, DataType::Float64);
        column_types.insert(col_b, DataType::Utf8);

        let bytes = write_overlay_parquet(&pending, &column_types).expect("write failed");
        let overlay = read_overlay_parquet(&bytes).expect("read failed");

        let row = overlay.updates.get(&row_id).expect("row not found");
        assert!(row.contains_key(&col_a));
        assert!(!row.contains_key(&col_b));  // col_b was not updated
    }
}
