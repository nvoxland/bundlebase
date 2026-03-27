//! Delete operation — records a tombstone file reference.
//!
//! `DeleteOp` stores only the tombstone filename. The actual row exclusion
//! happens at scan time in `DataBlock::scan()`, not in `apply_dataframe()`.

use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
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

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        // No-op: tombstones are loaded separately during Bundle::open()
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
