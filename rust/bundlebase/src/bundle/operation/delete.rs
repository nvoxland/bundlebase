//! Delete operation — records a tombstone file reference.
//!
//! `DeleteOp` stores only the tombstone filename. The tombstone keys rows by
//! block `(BlockId, version)` and row number (see `tombstone`). The actual row exclusion
//! happens at scan time in `DataBlock::scan()`, not in `apply_dataframe()`.

use crate::bundle::bundle_schema::BundleSchema;
use crate::bundle::operation::Operation;
use crate::bundle::tombstone;
use crate::data::VersionedBlockId;
use crate::object_id::ObjectId;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Operation that records a tombstone file for deleted rows.
///
/// The Puffin file contains deletion vectors keyed by block ID and version.
/// Row exclusion happens at the DataBlock/reader scan level, so `apply_dataframe()`
/// is a no-op.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOp {
    /// Filename of the tombstone file (e.g., "a3f8b2c1d4e5.overlay.tomb.puffin")
    pub tombstone: String,
    /// The WHERE clause(s) that produced this delete, stored for historical reference only.
    #[serde(rename = "where")]
    pub where_clause: String,
}

impl DeleteOp {
    pub fn new(tombstone: impl Into<String>, where_clause: impl Into<String>) -> Self {
        Self {
            tombstone: tombstone.into(),
            where_clause: where_clause.into(),
        }
    }
}

impl Operation for DeleteOp {
    fn describe(&self) -> String {
        format!("DELETE: {}", self.tombstone)
    }

    fn to_empty(&self, _context: &super::EmptyContext) -> Option<super::AnyOperation> {
        None
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify bundle has data to delete from
        let packs = bundle.packs();
        let has_data = packs
            .read()
            .values()
            .any(|p: &Arc<crate::bundle::Pack>| !p.is_empty());
        if !has_data {
            return Err("Cannot delete from an empty bundle".into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Load tombstone file and distribute deleted row numbers to DataBlocks
        let data_dir = bundle.data_dir();
        let tomb_file = data_dir
            .file(&self.tombstone)
            .map_err(|e| DataFusionError::External(e))?;
        let bytes = tomb_file
            .read_bytes()
            .await
            .map_err(|e| DataFusionError::External(e))?;

        let bytes = bytes.ok_or_else(|| {
            DataFusionError::Execution(format!("DELETE Puffin file not found: {}", self.tombstone))
        })?;
        let deletions = tombstone::deserialize(&bytes).map_err(|error| {
            DataFusionError::Execution(format!("Invalid DELETE file '{}': {error}", self.tombstone))
        })?;

        // Validate every matching block before mutating any of them. Stale
        // versions are skipped; their ordinals do not describe live content.
        let packs = bundle.packs().read().clone();
        let mut pending = Vec::new();
        if let Some(pack) = packs.get(&ObjectId::BASE_PACK) {
            let blocks: HashMap<_, _> = pack
                .blocks()
                .into_iter()
                .map(|block| (VersionedBlockId::new(*block.id(), block.version()), block))
                .collect();
            for deletion in &deletions {
                if let Some(block) = blocks.get(&deletion.block) {
                    if let (Some(rows), Some(last)) = (block.num_rows(), deletion.rows.max()) {
                        if last >= rows as u64 {
                            return Err(DataFusionError::Execution(format!(
                                "DELETE vector for {} references row {last} outside block with {rows} rows",
                                deletion.block
                            )));
                        }
                    }
                    pending.push((block.clone(), &deletion.rows));
                } else {
                    log::debug!(
                        "Skipping DELETE vector for inactive block {}",
                        deletion.block
                    );
                }
            }
        }
        let deleted_count: u64 = deletions.iter().map(|d| d.rows.len()).sum();
        for (block, rows) in pending {
            // deserialize checked every ordinal fits u32 before any expansion.
            block.add_deleted_rows(rows.iter().map(|row| row as u32));
        }

        log::debug!(
            "Loaded {} deleted rows from {}",
            deleted_count,
            self.tombstone
        );

        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        // No-op: deleted row filtering happens at the DataBlock/reader scan level,
        // before operations are applied to the DataFrame.
        Ok(df)
    }

    fn allowed_on_view(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = DeleteOp::new("a3f8b2c1d4e5.overlay.tomb.puffin", "id = 1");
        assert_eq!(op.describe(), "DELETE: a3f8b2c1d4e5.overlay.tomb.puffin");
    }

    #[test]
    fn test_serialization() {
        let op = DeleteOp::new("abc123def456.overlay.tomb.puffin", "salary < 0");
        let yaml = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(yaml.contains("tombstone"));
        assert!(yaml.contains("abc123def456.overlay.tomb.puffin"));
        assert!(yaml.contains("where"));
        assert!(yaml.contains("salary < 0"));

        let deserialized: DeleteOp = serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");
        assert_eq!(deserialized.tombstone, "abc123def456.overlay.tomb.puffin");
        assert_eq!(deserialized.where_clause, "salary < 0");
    }
}
