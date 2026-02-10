use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
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
    pub old_name: String,
    pub new_name: String,
}

impl RenameColumnOp {
    pub fn setup(old_name: &str, new_name: &str) -> Self {
        Self {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
        }
    }
}

#[async_trait]
impl Operation for RenameColumnOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let schema = bundle.schema().await?;
        schema.field_with_name(&self.old_name)?;

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
        let df = df
            .with_column_renamed(&self.old_name, &self.new_name)
            .map_err(|e| Box::new(e) as BundlebaseError)?;
        Ok(df)
    }

    fn describe(&self) -> String {
        format!("RENAME COLUMN: {} to {}", self.old_name, self.new_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = RenameColumnOp::setup("first_name", "fname");
        assert_eq!(op.describe(), "RENAME COLUMN: first_name to fname");
    }

    #[test]
    fn test_describe_multiple_cases() {
        let cases = vec![
            ("id", "identifier", "RENAME COLUMN: id to identifier"),
            ("title", "job_title", "RENAME COLUMN: title to job_title"),
            (
                "a",
                "very_long_column_name",
                "RENAME COLUMN: a to very_long_column_name",
            ),
        ];

        for (old, new, expected) in cases {
            let op = RenameColumnOp::setup(old, new);
            assert_eq!(op.describe(), expected);
        }
    }

    #[test]
    fn test_config_serialization() {
        let op = RenameColumnOp::setup("first_name", "fname");

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = r#"oldName: first_name
newName: fname
"#;
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_config_serialization_various() {
        let cases = vec![
            ("col1", "col2", "oldName: col1\nnewName: col2\n"),
            ("a", "b", "oldName: a\nnewName: b\n"),
            ("x_old", "y_new", "oldName: x_old\nnewName: y_new\n"),
        ];

        for (old, new, expected) in cases {
            let op = RenameColumnOp::setup(old, new);
            let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
            assert_eq!(serialized, expected);
        }
    }

    #[test]
    fn test_version_exact_value() {
        let op = RenameColumnOp::setup("first_name", "fname");
        let version = op.version();

        assert_eq!(version, "98c6271248e2");
    }
}
