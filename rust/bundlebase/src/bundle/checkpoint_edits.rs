//! Private checkpoint replay compaction. Public history retains original operations.
//!
//! Only consecutive edit operations are combined. Every other operation is a
//! barrier, including configuration changes and block replacements, so no check
//! or apply method observes edits earlier or later than it did during full replay.

use crate::bundle::operation::{AnyOperation, DeleteOp, UpdateDataOp};
use crate::bundle::update_overlay::{PendingUpdates, UpdateOverlay};
use crate::bundle::{tombstone, update_overlay};
use crate::object_id::ColumnId;
use crate::BundlebaseError;
use arrow::datatypes::DataType;
use bundlebase_common::{ContentAddress, ContentCategory, ContentFormat, VersionedBlockId};
use bundlebase_data::puffin::{self, DeletionVector};
use bundlebase_io::IOReadWriteDir;
use bytes::Bytes;
use datafusion::scalar::ScalarValue;
use std::collections::HashMap;

/// Compact consecutive edit runs without changing any intervening operation.
/// Missing update files retain the original run, matching normal replay's skip.
/// Invalid edit files return an error; callers must abandon that checkpoint.
pub(super) async fn consolidate_operations(
    ops: &[AnyOperation],
    data_dir: &dyn IOReadWriteDir,
) -> Result<Vec<AnyOperation>, BundlebaseError> {
    let mut result = Vec::with_capacity(ops.len());
    let mut start = 0;
    for (index, op) in ops.iter().enumerate() {
        if !is_edit(op) {
            append_run(&ops[start..index], data_dir, &mut result).await?;
            result.push(op.clone());
            start = index + 1;
        }
    }
    append_run(&ops[start..], data_dir, &mut result).await?;
    Ok(result)
}

fn is_edit(op: &AnyOperation) -> bool {
    matches!(op, AnyOperation::Delete(_) | AnyOperation::UpdateData(_))
}

async fn append_run(
    run: &[AnyOperation],
    data_dir: &dyn IOReadWriteDir,
    result: &mut Vec<AnyOperation>,
) -> Result<(), BundlebaseError> {
    if run.len() < 2 {
        result.extend_from_slice(run);
        return Ok(());
    }
    match compact_run(run, data_dir).await? {
        Some(compacted) => result.extend(compacted),
        None => result.extend_from_slice(run),
    }
    Ok(())
}

async fn compact_run(
    run: &[AnyOperation],
    data_dir: &dyn IOReadWriteDir,
) -> Result<Option<Vec<AnyOperation>>, BundlebaseError> {
    let mut deletions: HashMap<VersionedBlockId, DeletionVector> = HashMap::new();
    let mut overlays: HashMap<VersionedBlockId, Vec<UpdateOverlay>> = HashMap::new();
    let mut types: HashMap<ColumnId, DataType> = HashMap::new();
    let mut delete_clauses = Vec::new();
    let mut update_clauses = Vec::new();

    // Complete all input validation before publishing any compacted data.
    for op in run {
        match op {
            AnyOperation::Delete(delete) => {
                let bytes = data_dir
                    .file(&delete.tombstone)?
                    .read_bytes()
                    .await?
                    .ok_or_else(|| {
                        BundlebaseError::from(format!(
                            "DELETE Puffin file not found: {}",
                            delete.tombstone,
                        ))
                    })?;
                // Validate Bundlebase identity and ordinal bounds as normal replay does.
                let decoded = tombstone::deserialize(&bytes)?;
                let vectors = puffin::deserialize(&bytes)?;
                for (decoded, mut vector) in decoded.into_iter().zip(vectors) {
                    // Cardinality is recomputed by the deterministic Puffin writer.
                    vector.properties.remove("cardinality");
                    if let Some(existing) = deletions.get_mut(&decoded.block) {
                        // Do not discard unfamiliar properties or change provenance.
                        if existing.properties != vector.properties {
                            return Ok(None);
                        }
                        existing.positions |= &vector.positions;
                    } else {
                        deletions.insert(decoded.block, vector);
                    }
                }
                delete_clauses.push(delete.where_clause.as_str());
            }
            AnyOperation::UpdateData(update) => {
                let Some(bytes) = data_dir.file(&update.overlay)?.read_bytes().await? else {
                    return Ok(None);
                };
                for (block, overlay) in update_overlay::read_overlay_parquet(&bytes)? {
                    for (column, (values, _)) in &overlay.columns {
                        if let Some(previous) = types.get(column) {
                            // Merge's Arrow arrays must have matching types. Normal
                            // scan-time application handles casts; retain that path.
                            if previous != values.data_type() {
                                return Ok(None);
                            }
                        } else {
                            types.insert(*column, values.data_type().clone());
                        }
                    }
                    overlays.entry(block).or_default().push(overlay);
                }
                update_clauses.push(update.where_clause.as_str());
            }
            _ => return Err("Non-edit operation in checkpoint edit run".into()),
        }
    }

    let mut compacted = Vec::with_capacity(2);
    if !delete_clauses.is_empty() {
        let vectors: Vec<_> = deletions.into_values().collect();
        let bytes = Bytes::from(puffin::serialize(&vectors)?);
        let path = write_content(bytes, "tomb", ContentFormat::Puffin, data_dir).await?;
        compacted.push(DeleteOp::new(path, delete_clauses.join("; ")).into());
    }
    if !update_clauses.is_empty() {
        let mut pending = PendingUpdates::new();
        for (block, values) in overlays {
            let merged = UpdateOverlay::merge(&values)?;
            let mut rows = std::collections::BTreeMap::new();
            for (position, &row) in merged.row_numbers.iter().enumerate() {
                let mut cells = HashMap::new();
                for (column, (values, is_set)) in &merged.columns {
                    if is_set.value(position) {
                        cells.insert(*column, ScalarValue::try_from_array(values, position)?);
                    }
                }
                rows.insert(row, cells);
            }
            if !rows.is_empty() {
                pending.insert(block, rows);
            }
        }
        // An empty overlay is a valid no-op. Keep the original run rather than
        // asking the ordinary writer to encode a missing block/column schema.
        if pending.is_empty() || types.is_empty() {
            return Ok(None);
        }
        let bytes = update_overlay::write_overlay_parquet(&pending, &types)?;
        let path = write_content(bytes, "update", ContentFormat::Parquet, data_dir).await?;
        compacted.push(UpdateDataOp::new(path, update_clauses.join("; ")).into());
    }
    Ok(Some(compacted))
}

async fn write_content(
    bytes: Bytes,
    subtype: &str,
    format: ContentFormat,
    data_dir: &dyn IOReadWriteDir,
) -> Result<String, BundlebaseError> {
    let address = ContentAddress::with_sub_type(ContentCategory::Overlay, subtype, format)?;
    let source = futures::stream::iter([Ok::<_, std::io::Error>(bytes)]);
    let written = data_dir.write_stream(Box::pin(source), &address).await?;
    data_dir.relative_path(written.file.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::operation::SetNameOp;
    use crate::data::BlockId;
    use crate::test_utils::random_memory_dir;
    use std::collections::BTreeMap;

    fn block(version: &str) -> VersionedBlockId {
        VersionedBlockId::new(
            BlockId::try_from("0000000000000001").expect("block ID"),
            version.to_string(),
        )
    }

    async fn delete_file(
        dir: &dyn IOReadWriteDir,
        path: &str,
        rows: &[u32],
    ) -> Result<AnyOperation, BundlebaseError> {
        let key = block("v1");
        let mut selection = tombstone::RowSelection::default();
        for &row in rows {
            selection.insert(&key, row);
        }
        let locations = HashMap::from([(key.block, "data.csv".to_string())]);
        dir.writable_file(path)?
            .write(tombstone::serialize(&selection, &locations)?)
            .await?;
        Ok(DeleteOp::new(path, "id > 0").into())
    }

    #[tokio::test]
    async fn deletion_union_is_deterministic_and_stops_at_non_edit_operations(
    ) -> Result<(), BundlebaseError> {
        let dir = random_memory_dir();
        let first = delete_file(dir.as_ref(), "first.puffin", &[1, 2]).await?;
        let second = delete_file(dir.as_ref(), "second.puffin", &[2, 3]).await?;
        let barrier: AnyOperation = SetNameOp::setup("between edits").into();
        let ops = vec![
            first.clone(),
            second.clone(),
            barrier.clone(),
            first.clone(),
        ];
        let compacted = consolidate_operations(&ops, dir.as_ref()).await?;
        assert_eq!(compacted.len(), 3);
        assert_eq!(compacted[1], barrier);
        assert_eq!(compacted[2], first);
        let AnyOperation::Delete(delete) = &compacted[0] else {
            panic!("first operation must be DELETE");
        };
        let bytes = dir
            .file(&delete.tombstone)?
            .read_bytes()
            .await?
            .expect("Puffin exists");
        let decoded = tombstone::deserialize(&bytes)?;
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].rows.iter().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(consolidate_operations(&ops, dir.as_ref()).await?, compacted);
        Ok(())
    }

    #[tokio::test]
    async fn overlays_merge_per_cell_including_explicit_nulls() -> Result<(), BundlebaseError> {
        let dir = random_memory_dir();
        let column_a = ColumnId::try_from("0000000000000002")?;
        let column_b = ColumnId::try_from("0000000000000003")?;
        let types = HashMap::from([(column_a, DataType::Int64), (column_b, DataType::Int64)]);
        let before = HashMap::from([(
            block("v1"),
            BTreeMap::from([(
                1,
                HashMap::from([
                    (column_a, ScalarValue::Int64(Some(10))),
                    (column_b, ScalarValue::Int64(Some(20))),
                ]),
            )]),
        )]);
        let after = HashMap::from([(
            block("v1"),
            BTreeMap::from([
                (1, HashMap::from([(column_a, ScalarValue::Int64(None))])),
                (2, HashMap::from([(column_b, ScalarValue::Int64(Some(30)))])),
            ]),
        )]);
        let mut ops = Vec::new();
        for (path, values) in [("before.parquet", before), ("after.parquet", after)] {
            let bytes = update_overlay::write_overlay_parquet(&values, &types)?;
            dir.writable_file(path)?.write(bytes).await?;
            ops.push(UpdateDataOp::new(path, "id > 0").into());
        }
        let compacted = consolidate_operations(&ops, dir.as_ref()).await?;
        assert_eq!(compacted.len(), 1);
        let AnyOperation::UpdateData(update) = &compacted[0] else {
            panic!("expected one UPDATE DATA operation");
        };
        let bytes = dir
            .file(&update.overlay)?
            .read_bytes()
            .await?
            .expect("overlay exists");
        let decoded = update_overlay::read_overlay_parquet(&bytes)?;
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].1.row_numbers, vec![1, 2]);
        let merged = &decoded[0].1;
        let (a, mask_a) = &merged.columns[&column_a];
        let (b, mask_b) = &merged.columns[&column_b];
        assert!(mask_a.value(0));
        assert!(!mask_a.value(1));
        assert_eq!(ScalarValue::try_from_array(a, 0)?, ScalarValue::Int64(None));
        assert!(mask_b.value(0));
        assert!(mask_b.value(1));
        assert_eq!(
            ScalarValue::try_from_array(b, 0)?,
            ScalarValue::Int64(Some(20))
        );
        assert_eq!(
            ScalarValue::try_from_array(b, 1)?,
            ScalarValue::Int64(Some(30))
        );
        assert_eq!(consolidate_operations(&ops, dir.as_ref()).await?, compacted);
        Ok(())
    }

    #[tokio::test]
    async fn missing_updates_are_preserved_and_corrupt_deletes_fail() -> Result<(), BundlebaseError>
    {
        let dir = random_memory_dir();
        let delete = delete_file(dir.as_ref(), "valid.puffin", &[1]).await?;
        let ops = vec![
            delete.clone(),
            UpdateDataOp::new("missing.parquet", "id = 1").into(),
        ];
        assert_eq!(consolidate_operations(&ops, dir.as_ref()).await?, ops);
        dir.writable_file("invalid.puffin")?
            .write(Bytes::from_static(b"invalid"))
            .await?;
        let invalid = vec![delete, DeleteOp::new("invalid.puffin", "id = 2").into()];
        assert!(consolidate_operations(&invalid, dir.as_ref())
            .await
            .is_err());
        Ok(())
    }
}
