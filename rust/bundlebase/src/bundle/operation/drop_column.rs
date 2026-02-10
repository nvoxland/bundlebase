use crate::bundle::operation::Operation;
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
    pub names: Vec<String>,
}

impl DropColumnOp {
    pub fn setup(names: Vec<&str>) -> Self {
        Self {
            names: names.iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[async_trait]
impl Operation for DropColumnOp {
    fn describe(&self) -> String {
        format!("DROP COLUMN: {:?}", self.names)
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
        let names_slice: Vec<&str> = self.names.as_slice().iter().map(|x| x.as_str()).collect();
        Ok(df.drop_columns(names_slice.as_slice())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = DropColumnOp::setup(vec!["col1", "col2"]);
        assert_eq!(op.describe(), r#"DROP COLUMN: ["col1", "col2"]"#);
    }

    #[test]
    fn test_describe_single_column() {
        let op = DropColumnOp::setup(vec!["title"]);
        assert_eq!(op.describe(), r#"DROP COLUMN: ["title"]"#);
    }

    #[test]
    fn test_serialization() {
        let op = DropColumnOp::setup(vec!["col1", "col2"]);

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = r#"names:
- col1
- col2
"#;
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_serialization_single() {
        let op = DropColumnOp::setup(vec!["title"]);

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = r#"names:
- title
"#;
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_version() {
        let op = DropColumnOp::setup(vec!["title"]);
        let version = op.version();

        // Exact value for this specific config (first 12 chars of SHA256)
        assert_eq!(version, "bdd2c52a4e75");
    }
}
