use crate::bundle::operation::Operation;
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
pub struct DropColumnOp {
    pub id: ColumnId,
    pub name: String,
}

impl DropColumnOp {
    pub fn setup(id: ColumnId, name: &str) -> Self {
        Self {
            name: name.to_string(),
            id,
        }
    }
}

#[async_trait]
impl Operation for DropColumnOp {
    fn describe(&self) -> String {
        format!("DROP COLUMN: {}", self.name)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        _ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        Ok(df.drop_columns(&[self.name.as_str()])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = DropColumnOp::setup(ColumnId::generate(), "col1");
        assert_eq!(op.describe(), "DROP COLUMN: col1");
    }

    #[test]
    fn test_serialization() {
        let op = DropColumnOp::setup(ColumnId::generate(), "col1");

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.starts_with("id:"));
        assert!(serialized.contains("name: col1"));
    }

    #[test]
    fn test_version() {
        let op = DropColumnOp::setup(ColumnId::generate(), "title");
        let version = op.version();

        // Version now includes column ids so it varies per invocation
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12);
    }
}
