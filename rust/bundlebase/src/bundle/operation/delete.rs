//! Delete operation — records a tombstone file reference.
//!
//! `DeleteOp` stores only the tombstone filename. The actual row exclusion
//! happens at scan time in `DataBlock::scan()`, not in `apply_dataframe()`.

use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::operation::Operation;
use crate::bundle::{tombstone, META_DIR};
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
/// The tombstone file contains the RowIds of deleted rows in binary format.
/// Row exclusion happens at the DataBlock/reader scan level, so `apply_dataframe()`
/// is a no-op.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOp {
    /// Filename of the tombstone file (e.g., "a3f8b2c1d4e5.tomb")
    pub tombstone: String,
}

impl DeleteOp {
    pub fn new(tombstone: impl Into<String>) -> Self {
        Self {
            tombstone: tombstone.into(),
        }
    }
}

impl Operation for DeleteOp {
    fn describe(&self) -> String {
        format!("DELETE: {}", self.tombstone)
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
        let manifest_dir = bundle.data_dir().writable_subdir(META_DIR)
            .map_err(|e| DataFusionError::External(e))?;
        // Tombstone path may include content-addressed subdirectory (e.g., "5f/abc123.tomb")
        let tomb_file = if self.tombstone.contains('/') {
            let parts: Vec<&str> = self.tombstone.splitn(2, '/').collect();
            manifest_dir.subdir(parts[0])
                .map_err(|e| DataFusionError::External(e))?
                .file(parts[1])
                .map_err(|e| DataFusionError::External(e))?
        } else {
            manifest_dir.file(&self.tombstone)
                .map_err(|e| DataFusionError::External(e))?
        };
        let bytes = tomb_file.read_bytes().await
            .map_err(|e| DataFusionError::External(e))?;

        let bytes = match bytes {
            Some(b) => b,
            None => {
                log::warn!("Tombstone file not found: {}", self.tombstone);
                return Ok(());
            }
        };

        let row_ids = tombstone::deserialize_tombstone(&bytes)
            .map_err(|e| DataFusionError::External(e))?;

        // Group RowIds by block_ref -> row numbers
        let mut by_block: HashMap<u16, Vec<u32>> = HashMap::new();
        for rid in &row_ids {
            by_block.entry(rid.block_ref().as_u16()).or_default().push(rid.row_number());
        }

        // Distribute to the corresponding DataBlocks in the base pack
        let packs = bundle.packs().read().clone();
        if let Some(pack) = packs.get(&ObjectId::BASE_PACK) {
            let blocks = pack.blocks();
            for (block_idx, row_numbers) in by_block {
                if let Some(block) = blocks.get(block_idx as usize) {
                    block.add_deleted_rows(row_numbers.into_iter());
                }
            }
        }

        log::debug!(
            "Loaded {} tombstoned RowIds from {}",
            row_ids.len(),
            self.tombstone
        );

        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _column_names: &mut ColumnNames,
    ) -> Result<DataFrame, BundlebaseError> {
        // No-op: tombstone filtering happens at the DataBlock/reader scan level,
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
        let op = DeleteOp::new("a3f8b2c1d4e5.tomb");
        assert_eq!(op.describe(), "DELETE: a3f8b2c1d4e5.tomb");
    }

    #[test]
    fn test_serialization() {
        let op = DeleteOp::new("abc123def456.tomb");
        let yaml = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(yaml.contains("tombstone"));
        assert!(yaml.contains("abc123def456.tomb"));

        let deserialized: DeleteOp =
            serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");
        assert_eq!(deserialized.tombstone, "abc123def456.tomb");
    }
}
