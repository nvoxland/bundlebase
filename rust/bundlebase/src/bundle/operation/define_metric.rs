use crate::bundle::operation::Operation;
use crate::bundle::BundleFacade;
use arrow::array::RecordBatch;
use datafusion::prelude::{SessionConfig, SessionContext};
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl DefineMetricOp {
    pub fn setup(name: &str, expression: &str, description: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            expression: expression.to_string(),
            description: description.map(|s| s.to_string()),
        }
    }
}

impl DefineMetricOp {
    /// Plan (without executing) `SELECT (<expression>) FROM bundle` against an
    /// empty batch with the bundle's internal (`col_<id>`) schema, with the
    /// bundle's functions available, so unknown columns, unknown functions and
    /// syntax errors fail at definition time.
    async fn validate_expression(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
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
        let empty = RecordBatch::new_empty(bundle.internal_schema().await?);
        ctx.register_batch("bundle", empty).map_err(|e| invalid(&e))?;

        let sql = format!("SELECT ({}) AS metric_value FROM bundle", self.expression);
        let state = ctx.state();
        Box::pin(state.create_logical_plan(&sql))
            .await
            .map_err(|e| invalid(&e))?;
        Ok(())
    }
}

impl Operation for DefineMetricOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        if self.name.trim().is_empty() {
            return Err("Metric name cannot be empty".into());
        }
        if self.expression.trim().is_empty() {
            return Err(format!("Metric '{}' has an empty expression", self.name).into());
        }
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
        let op = DefineMetricOp::setup("total_revenue", "SUM(amount)", None);
        assert_eq!(op.describe(), "DEFINE METRIC: total_revenue = SUM(amount)");
    }

    #[test]
    fn test_version() {
        let op = DefineMetricOp::setup("n", "COUNT(*)", None);
        assert_eq!(op.version().len(), 12);
        assert_eq!(op.version(), DefineMetricOp::setup("n", "COUNT(*)", None).version());
        assert_ne!(op.version(), DefineMetricOp::setup("n", "SUM(x)", None).version());
        assert_ne!(
            op.version(),
            DefineMetricOp::setup("n", "COUNT(*)", Some("d")).version()
        );
    }

    #[test]
    fn test_serialization_roundtrip_with_description() {
        let op = DefineMetricOp::setup(
            "aov",
            "SUM(amount) / COUNT(DISTINCT order_id)",
            Some("average order value"),
        );
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let back: DefineMetricOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(op, back);
    }

    #[test]
    fn test_serialization_roundtrip_without_description() {
        let op = DefineMetricOp::setup("n", "COUNT(*)", None);
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        // description omitted when None.
        assert!(!yaml.contains("description"));
        let back: DefineMetricOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(op, back);
    }
}
