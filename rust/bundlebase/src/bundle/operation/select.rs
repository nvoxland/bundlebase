use crate::bundle::operation::Operation;
use crate::{BundlebaseError, Bundle};
use crate::metrics::{OperationTimer, OperationCategory, OperationOutcome, start_span};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::bundle::sql::with_temp_table;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SelectOp {
    /// List of column expressions to select (can include function calls, AS aliases, etc.)
    pub columns: Vec<String>,
}

impl SelectOp {
    pub async fn setup(columns: &Vec<&str>) -> Result<Self, BundlebaseError> {
        if columns.is_empty() {
            return Err(Box::new(DataFusionError::Plan(
                "SELECT requires at least one column".to_string(),
            )) as BundlebaseError);
        }

        Ok(Self {
            columns: columns.iter().map(|s| s.to_string()).collect(),
        })
    }
}

#[async_trait]
impl Operation for SelectOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, _bundle: &mut Bundle) -> Result<(), DataFusionError> {
        // Select doesn't change the schema structure, just filters columns
        // Schema will be updated during apply_dataframe
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        let mut span = start_span(OperationCategory::Query, "select");
        span.set_attribute("column_count", &self.columns.len().to_string());
        span.set_attribute("columns", &self.columns.join(", "));

        let timer = OperationTimer::start(OperationCategory::Query, "select")
            .with_label("column_count", &self.columns.len().to_string());

        let columns = self.columns.clone();
        let ctx_for_closure = ctx.clone();

        let result = with_temp_table(&ctx, df, |table_name| {
            async move {
                let column_list = columns.join(", ");
                let sql = format!("SELECT {} FROM {}", column_list, table_name);
                ctx_for_closure.sql(&sql)
                    .await
                    .map_err(|e| Box::new(e) as BundlebaseError)
            }
        })
        .await;

        match &result {
            Ok(_) => {
                span.set_outcome(OperationOutcome::Success);
                timer.finish(OperationOutcome::Success);
            }
            Err(e) => {
                span.record_error(&e.to_string());
                timer.finish(OperationOutcome::Error);
            }
        }

        result
    }

    fn describe(&self) -> String {
        format!("SELECT: {}", self.columns.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = SelectOp {
            columns: vec!["id".to_string(), "name AS full_name".to_string()],
        };
        assert_eq!(op.describe(), "SELECT: id, name AS full_name");
    }

    #[test]
    fn test_describe_with_functions() {
        let op = SelectOp {
            columns: vec![
                "id".to_string(),
                "UPPER(name) AS name_upper".to_string(),
                "salary * 2 AS doubled_salary".to_string(),
            ],
        };
        let desc = op.describe();
        assert!(desc.contains("UPPER(name)"));
        assert!(desc.contains("doubled_salary"));
    }

    #[test]
    fn test_config_serialization() {
        let op = SelectOp {
            columns: vec![
                "id".to_string(),
                "name AS full_name".to_string(),
                "salary * 1.5".to_string(),
            ],
        };

        // Verify serialization is possible
        let serialized = serde_yaml::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("columns"));
        assert!(serialized.contains("full_name"));
        assert!(serialized.contains("salary"));

        // Verify we can deserialize back
        let deserialized: SelectOp =
            serde_yaml::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized.columns.len(), 3);
        assert_eq!(deserialized.columns[0], "id");
    }

    #[test]
    fn test_version() {
        let op = SelectOp {
            columns: vec!["id".to_string(), "name".to_string()],
        };
        let version = op.version();
        // Just verify it returns a version string
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12); // SHA256 short hash format
    }
}
