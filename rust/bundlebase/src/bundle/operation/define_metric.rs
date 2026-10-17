use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use crate::{Bundle, BundlebaseError};
use datafusion::common::{DFSchema, DataFusionError};
use datafusion::logical_expr::ExprSchemable;
use datafusion::prelude::{SessionConfig, SessionContext};
use serde::{Deserialize, Serialize};

/// A resolved metric: a named, reusable aggregate expression plus optional
/// description. Metrics are the "how to compute the numbers" half of the
/// semantic layer — surfacing them to an agent means everyone computes
/// "revenue" the same way, instead of each query re-inventing the SUM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricDefinition {
    pub name: String,
    pub expression: String,
    pub description: Option<String>,
}

/// Define (or redefine) a named metric on the bundle. Metadata-only: it records
/// a canonical measure expression (e.g. `SUM(amount)`) that travels in the
/// manifest and is surfaced to agents/tools; it does not transform data.
/// Resolved by replaying operations (last write wins per metric name).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DefineMetricOp {
    pub name: String,
    pub expression: String,
}

impl DefineMetricOp {
    pub fn setup(name: &str, expression: &str) -> Self {
        Self {
            name: name.to_string(),
            expression: expression.to_string(),
        }
    }
}

impl DefineMetricOp {
    /// Resolve and type-check against the internal schema and registered
    /// functions without executing a query or consulting a table provider.
    pub(crate) async fn validate_expression(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let invalid = |e: &dyn std::fmt::Display| -> BundlebaseError {
            format!(
                "Invalid expression for metric '{}': {}",
                self.name,
                bundle.display_sql(&e.to_string())
            )
            .into()
        };

        let mut config = SessionConfig::new();
        config.options_mut().sql_parser.enable_ident_normalization = false;
        let ctx = SessionContext::new_with_config(config);
        let state = bundle.ctx().state();
        for udf in state.scalar_functions().values() {
            ctx.register_udf(udf.as_ref().clone());
        }
        for udaf in state.aggregate_functions().values() {
            ctx.register_udaf(udaf.as_ref().clone());
        }
        for udwf in state.window_functions().values() {
            ctx.register_udwf(udwf.as_ref().clone());
        }
        let schema = bundle.internal_schema().await?;
        let schema = DFSchema::try_from_qualified_schema("bundle", schema.as_ref())
            .map_err(|e| invalid(&e))?;
        // Resolve and type-check the expression directly. Planning a synthetic
        // SELECT unnecessarily enters the table/relation planner; semantic
        // validation only needs the schema and registered functions.
        let expression = ctx
            .parse_sql_expr(&self.expression, &schema)
            .map_err(|e| invalid(&e))?;
        expression.get_type(&schema).map_err(|e| invalid(&e))?;
        Ok(())
    }
}

impl Operation for DefineMetricOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        super::semantic_validation::check_expression(&self.name, &self.expression)?;
        // Boxed: planning futures are large and this runs nested inside the
        // builder's command/apply futures.
        Box::pin(self.validate_expression(bundle)).await
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        // Metadata-only; metrics are resolved from the operation list.
        Ok(())
    }

    fn describe(&self) -> String {
        format!("DEFINE METRIC: {} = {}", self.name, self.expression)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = DefineMetricOp::setup("total_revenue", "SUM(amount)");
        assert_eq!(op.describe(), "DEFINE METRIC: total_revenue = SUM(amount)");
    }

    #[test]
    fn test_version() {
        let op = DefineMetricOp::setup("n", "COUNT(*)");
        assert_eq!(op.version().len(), 12);
        assert_eq!(
            op.version(),
            DefineMetricOp::setup("n", "COUNT(*)").version()
        );
        assert_ne!(op.version(), DefineMetricOp::setup("n", "SUM(x)").version());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let op = DefineMetricOp::setup("aov", "SUM(amount) / COUNT(DISTINCT order_id)");
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        assert!(!yaml.contains("description"));
        let back: DefineMetricOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(op, back);
    }
}
