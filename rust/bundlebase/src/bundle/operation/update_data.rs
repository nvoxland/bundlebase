//! Update data operation — records an overlay parquet file reference.
//!
//! `UpdateDataOp` stores the filename of an overlay parquet file containing
//! updated values. The actual value merging happens at scan time in
//! `DataBlock::scan()`, not in `apply_dataframe()`.

use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::operation::Operation;
use crate::bundle::{update_overlay, META_DIR};
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Operation that records an overlay parquet file for updated values.
///
/// The overlay file contains RowIds, updated column values (keyed by ColumnId),
/// and a bitmask indicating which columns were actually SET. Value merging
/// happens at the DataBlock scan level, so `apply_dataframe()` is a no-op.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDataOp {
    /// Filename of the overlay parquet file (e.g., "a3f8b2c1d4e5.update")
    pub overlay: String,
}

impl UpdateDataOp {
    pub fn new(overlay: impl Into<String>) -> Self {
        Self {
            overlay: overlay.into(),
        }
    }
}

impl Operation for UpdateDataOp {
    fn describe(&self) -> String {
        format!("UPDATE DATA: {}", self.overlay)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let packs = bundle.packs();
        let has_data = packs
            .read()
            .values()
            .any(|p: &Arc<crate::bundle::Pack>| !p.is_empty());
        if !has_data {
            return Err("Cannot update an empty bundle".into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let manifest_dir = bundle.data_dir().writable_subdir(META_DIR)
            .map_err(|e| DataFusionError::External(e))?;
        // Overlay path may include content-addressed subdirectory (e.g., "52/abc123.update")
        let overlay_file = if self.overlay.contains('/') {
            let parts: Vec<&str> = self.overlay.splitn(2, '/').collect();
            manifest_dir.subdir(parts[0])
                .map_err(|e| DataFusionError::External(e))?
                .file(parts[1])
                .map_err(|e| DataFusionError::External(e))?
        } else {
            manifest_dir.file(&self.overlay)
                .map_err(|e| DataFusionError::External(e))?
        };
        let bytes = overlay_file.read_bytes().await
            .map_err(|e| DataFusionError::External(e))?;

        let bytes = match bytes {
            Some(b) => b,
            None => {
                log::warn!("Update overlay file not found: {}", self.overlay);
                return Ok(());
            }
        };

        let overlay = update_overlay::read_overlay_parquet(&bytes)
            .map_err(|e| DataFusionError::External(e))?;

        let total_rows = overlay.updates.len();

        // Distribute overlay entries to corresponding DataBlocks by block_ref
        let mut by_block: std::collections::HashMap<u16, update_overlay::UpdateOverlay> = std::collections::HashMap::new();
        for (row_id, cell_updates) in overlay.updates {
            let block_idx = row_id.block_ref().as_u16();
            let block_overlay = by_block.entry(block_idx).or_insert_with(|| update_overlay::UpdateOverlay {
                updates: std::collections::HashMap::new(),
            });
            block_overlay.updates.insert(row_id, cell_updates);
        }

        let packs = bundle.packs().read().clone();
        if let Some(pack) = packs.get(&crate::object_id::ObjectId::BASE_PACK) {
            let blocks = pack.blocks();
            for (block_idx, block_overlay) in by_block {
                if let Some(block) = blocks.get(block_idx as usize) {
                    block.add_update_overlay(block_overlay);
                }
            }
        }

        log::debug!(
            "Loaded {} update overlay rows from {}",
            total_rows,
            self.overlay
        );

        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _column_names: &mut ColumnNames,
    ) -> Result<DataFrame, BundlebaseError> {
        // No-op: overlay merging happens at the DataBlock scan level
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
        let op = UpdateDataOp::new("abc123.update");
        assert_eq!(op.describe(), "UPDATE DATA: abc123.update");
    }

    #[test]
    fn test_serialization() {
        let op = UpdateDataOp::new("abc123def456.update");
        let yaml = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(yaml.contains("overlay"));
        assert!(yaml.contains("abc123def456.update"));

        let deserialized: UpdateDataOp =
            serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");
        assert_eq!(deserialized.overlay, "abc123def456.update");
    }
}
