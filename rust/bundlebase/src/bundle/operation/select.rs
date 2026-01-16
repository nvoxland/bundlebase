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
pub struct SelectOp {
    pub sql: String,
    pub parameters: Vec<ParameterValue>,
}

impl SelectOp {
    pub async fn setup(sql: String, parameters: Vec<ScalarValue>) -> Result<Self, BundlebaseError> {
        // Substitute parameters into SQL for schema inference
        let mut substituted_sql = sql.clone();
        for (i, param) in parameters.iter().enumerate() {
            let placeholder = format!("${}", i + 1);
            let value_str = crate::bundle::scalar_value_to_sql_literal(param);
            substituted_sql = substituted_sql.replace(&placeholder, &value_str);
        }

        Ok(Self {
            sql,
            parameters: parameters.into_iter().map(ParameterValue::from).collect(),
        })
    }
}

#[async_trait]
impl Operation for SelectOp {
    fn describe(&self) -> String {
        self.sql.clone()
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, _bundle: &mut Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
    ) -> Result<DataFrame, BundlebaseError> {
        let mut span = start_span(OperationCategory::Select, "sql");
        span.set_attribute("sql", &self.sql);
        span.set_attribute("param_count", self.parameters.len().to_string());

        let timer = OperationTimer::start(OperationCategory::Select, "sql");

        let user_sql = self.sql.clone();
        let parameters = self.parameters.clone();
        let ctx_for_closure = ctx.clone();

        let result = with_temp_table(&ctx, df, |table_name| {
            async move {
                // Substitute parameters into SQL
                let mut sql = user_sql;
                for (i, param) in parameters.iter().enumerate() {
                    let placeholder = format!("${}", i + 1);
                    let value_str =
                        crate::bundle::scalar_value_to_sql_literal(&param.to_scalar_value());
                    sql = sql.replace(&placeholder, &value_str);
                }

                // Replace "bundle" references with table_name in user SQL
                sql = sql.replace("bundle", &table_name);

                // Execute the SQL query
                ctx_for_closure
                    .sql(&sql)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let sql = "SELECT * FROM bundle WHERE salary > $1";
        let op = SelectOp {
            sql: sql.to_string(),
            parameters: vec![ParameterValue::Float64(50000.0)],
        };
        assert_eq!(op.describe(), format!("{}", sql));
    }

    #[test]
    fn test_config_serialization() {
        let sql = "SELECT * FROM bundle WHERE salary > $1 AND name = $2";
        let config = SelectOp {
            sql: sql.to_string(),
            parameters: vec![
                ParameterValue::Float64(50000.0),
                ParameterValue::String("USA".to_string()),
            ],
        };

        // Verify serialization is possible
        let serialized = serde_yaml::to_string(&config).expect("Failed to serialize");
        assert!(serialized.contains("sql"));
        assert!(serialized.contains("parameters"));
        assert!(serialized.contains("float64") || serialized.contains("50000"));
        assert!(serialized.contains("string") || serialized.contains("USA"));

        // Verify we can deserialize back
        let deserialized: SelectOp =
            serde_yaml::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized.sql, sql);
        assert_eq!(deserialized.parameters.len(), 2);
    }

    #[test]
    fn test_version() {
        let op = SelectOp {
            sql: "SELECT * FROM bundle".to_string(),
            parameters: vec![],
        };
        let version = op.version();
        // Just verify it returns a version string
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12); // SHA256 short hash format
    }
}
