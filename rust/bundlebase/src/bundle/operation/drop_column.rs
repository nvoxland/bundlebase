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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropColumnOp {
    pub id: ColumnId,
}

impl DropColumnOp {
    pub fn setup(id: ColumnId) -> Self {
        Self { id }
    }
}

impl Operation for DropColumnOp {
    fn describe(&self) -> String {
        format!("DROP COLUMN: {}", self.id)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        bundle.column_name(&self.id)
            .ok_or_else(|| BundlebaseError::from(format!("Column with ID '{}' not found", self.id)))?;
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        let col = bundle_schema.internal_column(&self.id)?;
        bundle_schema.remove(&self.id);
        Ok(df.drop_columns(&[col])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let id = ColumnId::generate();
        let op = DropColumnOp::setup(id);
        assert!(op.describe().starts_with("DROP COLUMN:"));
    }

    #[test]
    fn test_serialization() {
        let op = DropColumnOp::setup(ColumnId::generate());

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.starts_with("id:"));
    }

    #[test]
    fn test_version() {
        let op = DropColumnOp::setup(ColumnId::generate());
        let version = op.version();

        // Version now includes column ids so it varies per invocation
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12);
    }
}
