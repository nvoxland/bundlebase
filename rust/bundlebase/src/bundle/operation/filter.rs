use crate::bundle::bundle_schema::BundleSchema;
use crate::bundle::operation::parameter_value::ParameterValue;
use crate::bundle::operation::Operation;
use crate::catalog::BundleViewTable;
use crate::metrics::{start_span, OperationCategory, OperationOutcome, OperationTimer};
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use datafusion::dataframe::DataFrame;
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion::scalar::ScalarValue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FilterOp {
    pub query: String,
    pub parameters: Vec<ParameterValue>,
}

impl FilterOp {
    /// Create a new FilterOp with a full SELECT statement.
    pub fn new(query: impl Into<String>, parameters: Vec<ScalarValue>) -> Self {
        Self {
            query: query.into(),
            parameters: parameters.into_iter().map(ParameterValue::from).collect(),
        }
    }
}

impl Operation for FilterOp {
    fn describe(&self) -> String {
        format!("FILTER: {}", self.query)
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        // Filter doesn't change the schema, so no reconfiguration needed
        Ok(())
    }

    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError> {
        use crate::bundle::bundle_schema;

        let mut span = start_span(OperationCategory::Select, "filter");
        span.set_attribute("sql", &self.query);
        span.set_attribute("param_count", self.parameters.len().to_string());

        let timer = OperationTimer::start(OperationCategory::Select, "filter");

        // Translate user-visible column names in the SQL to stable internal names
        let translated_query = bundle_schema.translate_sql(&self.query);

        let mut config = SessionConfig::new();
        config.options_mut().sql_parser.enable_ident_normalization = false;
        let filter_ctx = SessionContext::new_with_config_rt(config, ctx.runtime_env());

        filter_ctx.register_table("bundle", Arc::new(BundleViewTable::new(df)))?;

        let params: Vec<ScalarValue> = self
            .parameters
            .iter()
            .map(|p| p.to_scalar_value())
            .collect();

        let result = async {
            let plan = filter_ctx
                .state()
                .create_logical_plan(&translated_query)
                .await
                .map_err(|e| Box::new(e) as BundlebaseError)?;

            let plan = plan
                .with_param_values(params)
                .map_err(|e| Box::new(e) as BundlebaseError)?;

            filter_ctx
                .execute_logical_plan(plan)
                .await
                .map_err(|e| Box::new(e) as BundlebaseError)
        }
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
        let query = "SELECT * FROM bundle WHERE id > 100";
        let op = FilterOp::new(query, vec![]);
        assert_eq!(op.describe(), format!("FILTER: {}", query));
    }

    #[test]
    fn test_describe_with_parameters() {
        let query = "SELECT * FROM bundle WHERE salary > $1";
        let op = FilterOp::new(query, vec![ScalarValue::Float64(Some(50000.0))]);
        assert_eq!(op.describe(), format!("FILTER: {}", query));
    }

    #[test]
    fn test_config_with_parameters() {
        let query = "SELECT * FROM bundle WHERE salary > $1 AND department = $2";
        let op = FilterOp::new(
            query,
            vec![
                ScalarValue::Float64(Some(50000.0)),
                ScalarValue::Utf8(Some("Engineering".to_string())),
            ],
        );

        // Verify serialization is possible
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert!(serialized.contains("query")); // camelCase due to serde rename_all
        assert!(serialized.contains("parameters"));
        assert!(serialized.contains("float64") || serialized.contains("50000"));
        assert!(serialized.contains("string") || serialized.contains("Engineering"));

        // Verify we can deserialize back
        let deserialized: FilterOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized.query, query);
        assert_eq!(deserialized.parameters.len(), 2);
    }

    #[test]
    fn test_version() {
        let op = FilterOp::new("SELECT * FROM bundle WHERE active = true", vec![]);
        let version = op.version();
        // Just verify it returns a version string
        assert!(!version.is_empty());
        assert_eq!(version.len(), 12); // SHA256 short hash format
    }
}
