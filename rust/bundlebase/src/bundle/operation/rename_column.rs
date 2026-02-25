use crate::bundle::column_metadata::ColumnNames;
use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use crate::object_id::ColumnId;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenameColumnOp {
    pub id: ColumnId,
    pub new_name: String,
}

impl RenameColumnOp {
    pub fn setup(id: ColumnId, new_name: &str) -> Self {
        Self {
            id,
            new_name: new_name.to_string(),
        }
    }
}

#[async_trait]
impl Operation for RenameColumnOp {
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
        column_names: &mut ColumnNames,
    ) -> Result<DataFrame, BundlebaseError> {
        let old_name = column_names.get(&self.id)
            .ok_or_else(|| BundlebaseError::from(format!("Column with ID '{}' not found", self.id)))?
            .clone();

        let df = df
            .with_column_renamed(&old_name, &self.new_name)
            .map_err(|e| Box::new(e) as BundlebaseError)?;
        column_names.insert(self.id, self.new_name.clone());
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("RENAME COLUMN: {} to {}", self.id, self.new_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = RenameColumnOp::setup(ColumnId::generate(), "fname");
        assert!(op.describe().contains("to fname"));
    }

    #[test]
    fn test_config_serialization() {
        let op = RenameColumnOp::setup(ColumnId::generate(), "fname");

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.starts_with("id: "));
        assert!(serialized.contains("newName: fname"));
    }

    #[test]
    fn test_version_exact_value() {
        let op = RenameColumnOp::setup(ColumnId::generate(), "fname");
        let version = op.version();

        // Version now includes column id so it varies per invocation
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12);
    }
}
