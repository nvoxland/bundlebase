use crate::bundle::operation::parameter_value::ParameterValue;
use crate::bundle::operation::Operation;
use crate::bundle::sql::with_temp_table;
use crate::metrics::{start_span, OperationCategory, OperationOutcome, OperationTimer};
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::SessionContext;
use datafusion::scalar::ScalarValue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FilterOp {
    pub where_clause: String,
    pub parameters: Vec<ParameterValue>,
}

impl FilterOp {
    pub async fn setup(
        where_clause: &str,
        parameters: Vec<ScalarValue>,
    ) -> Result<Self, BundlebaseError> {
        Ok(Self {
            where_clause: where_clause.to_string(),
            parameters: parameters.into_iter().map(ParameterValue::from).collect(),
        })
    }
}

#[async_trait]
impl Operation for FilterOp {
    fn describe(&self) -> String {
        format!("FILTER: WHERE {}", self.where_clause)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, _bundle: &mut Bundle) -> Result<(), DataFusionError> {
        // Filter doesn't change the schema, so no reconfiguration needed
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        let mut span = start_span(OperationCategory::Select, "filter");
        span.set_attribute("expression", &self.where_clause);

        let timer = OperationTimer::start(OperationCategory::Select, "filter")
            .with_label("expression", &self.where_clause);

        // Build the filter expression with parameter substitution
        let where_clause = self.where_clause.clone();
        let parameters = self.parameters.clone();
        let ctx_for_closure = ctx.clone();

        let result = with_temp_table(&ctx, df, |temp_table| async move {
            let mut substituted_clause = where_clause;
            for (i, param) in parameters.iter().enumerate() {
                let placeholder = format!("${}", i + 1);
                let value_str =
                    crate::bundle::scalar_value_to_sql_literal(&param.to_scalar_value());
                substituted_clause = substituted_clause.replace(&placeholder, &value_str);
            }

            let sql = format!("SELECT * FROM {} WHERE {}", temp_table, substituted_clause);
            ctx_for_closure
                .sql(&sql)
                .await
                .map_err(|e| Box::new(e) as BundlebaseError)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let clause = "id > 100 AND name = 'Alice'";
        let op = FilterOp {
            where_clause: clause.to_string(),
            parameters: vec![],
        };
        assert_eq!(op.describe(), format!("FILTER: WHERE {}", clause));
    }

    #[test]
    fn test_describe_with_parameters() {
        let clause = "salary > $1";
        let op = FilterOp {
            where_clause: clause.to_string(),
            parameters: vec![ParameterValue::Float64(50000.0)],
        };
        assert_eq!(op.describe(), format!("FILTER: WHERE {}", clause));
    }

    #[test]
    fn test_config_with_parameters() {
        let clause = "salary > $1 AND department = $2";
        let op = FilterOp {
            where_clause: clause.to_string(),
            parameters: vec![
                ParameterValue::Float64(50000.0),
                ParameterValue::String("Engineering".to_string()),
            ],
        };

        // Verify serialization is possible
        let serialized = serde_yaml::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("whereClause")); // camelCase due to serde rename_all
        assert!(serialized.contains("parameters"));
        assert!(serialized.contains("float64") || serialized.contains("50000"));
        assert!(serialized.contains("string") || serialized.contains("Engineering"));

        // Verify we can deserialize back
        let deserialized: FilterOp =
            serde_yaml::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized.where_clause, clause);
        assert_eq!(deserialized.parameters.len(), 2);
    }

    #[test]
    fn test_version() {
        let op = FilterOp {
            where_clause: "active = true".to_string(),
            parameters: vec![],
        };
        let version = op.version();
        // Just verify it returns a version string
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12); // SHA256 short hash format
    }
}
