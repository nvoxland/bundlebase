use crate::bundle::bundle_schema::BundleSchema;
use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Records that the most recent active cast on a column was dropped.
///
/// This op is always a no-op in `apply_dataframe`: `resolve_cast_ops` cancels
/// the corresponding `CastColumnOp` (and this op itself) before the pipeline
/// runs, so neither the cast nor the drop is applied to the DataFrame.
///
/// Storing only the column ID (not a `revert_to` type) means the operation
/// remains correct even if the ops list is reordered — the actual revert
/// type is determined dynamically by `resolve_cast_ops` at pipeline time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropCastColumnOp {
    pub id: ColumnId,
}

impl DropCastColumnOp {
    pub fn setup(id: ColumnId) -> Self {
        Self { id }
    }
}

impl Operation for DropCastColumnOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        bundle.column_name(&self.id).ok_or_else(|| {
            BundlebaseError::from(format!("Column with ID '{}' not found", self.id))
        })?;
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    /// Always a no-op: `resolve_cast_ops` skips this op before the pipeline runs.
    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        _bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("DROP CAST COLUMN: {}", self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = DropCastColumnOp::setup(ColumnId::generate());
        assert!(op.describe().contains("DROP CAST COLUMN"));
    }

    #[test]
    fn test_config_serialization() {
        let id = ColumnId::generate();
        let op = DropCastColumnOp::setup(id);
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let deserialized: DropCastColumnOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }
}
