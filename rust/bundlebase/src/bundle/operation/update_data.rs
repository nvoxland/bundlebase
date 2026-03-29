//! Update data operation — records an overlay parquet file reference.
//!
//! `UpdateDataOp` stores the filename of an overlay parquet file containing
//! updated values. The actual value merging happens at scan time in
//! `DataBlock::scan()`, not in `apply_dataframe()`.

use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::operation::Operation;
use crate::bundle::update_overlay;
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
    /// The WHERE clause(s) that produced this update, stored for historical reference only.
    #[serde(rename = "where")]
    pub where_clause: String,
}

impl UpdateDataOp {
    pub fn new(overlay: impl Into<String>, where_clause: impl Into<String>) -> Self {
        Self {
            overlay: overlay.into(),
            where_clause: where_clause.into(),
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
        let data_dir = bundle.data_dir();
        let overlay_file = data_dir.file(&self.overlay)
            .map_err(|e| DataFusionError::External(e))?;
        let bytes = overlay_file.read_bytes().await
            .map_err(|e| DataFusionError::External(e))?;

        let bytes = match bytes {
            Some(b) => b,
            None => {
                log::warn!("Update overlay file not found: {}", self.overlay);
                return Ok(());
            }
        };

        // read_overlay_parquet returns per-block overlays directly from row groups
        let block_overlays = update_overlay::read_overlay_parquet(&bytes)
            .map_err(|e| DataFusionError::External(e))?;

        let mut total_rows = 0;
        let packs = bundle.packs().read().clone();
        if let Some(pack) = packs.get(&crate::object_id::ObjectId::BASE_PACK) {
            let blocks = pack.blocks();
            for (block_idx, overlay) in block_overlays {
                total_rows += overlay.row_numbers.len();
                if let Some(block) = blocks.get(block_idx as usize) {
                    block.add_update_overlay(overlay);
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
        let op = UpdateDataOp::new("abc123.update", "id = 1");
        assert_eq!(op.describe(), "UPDATE DATA: abc123.update");
    }

    #[test]
    fn test_serialization() {
        let op = UpdateDataOp::new("abc123def456.update", "department = 'eng'");
        let yaml = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(yaml.contains("overlay"));
        assert!(yaml.contains("abc123def456.update"));
        assert!(yaml.contains("where"));
        assert!(yaml.contains("department = 'eng'"));

        let deserialized: UpdateDataOp =
            serde_yaml_ng::from_str(&yaml).expect("Failed to deserialize");
        assert_eq!(deserialized.overlay, "abc123def456.update");
        assert_eq!(deserialized.where_clause, "department = 'eng'");
    }
}
