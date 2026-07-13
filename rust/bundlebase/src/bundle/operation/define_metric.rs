use crate::bundle::operation::Operation;
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

impl Operation for DefineMetricOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        if self.name.trim().is_empty() {
            return Err("Metric name cannot be empty".into());
        }
        if self.expression.trim().is_empty() {
            return Err(format!("Metric '{}' has an empty expression", self.name).into());
        }
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        // Metadata-only; metrics are resolved from the operation list.
        Ok(())
    }

    fn describe(&self) -> String {
        format!("CREATE METRIC: {} = {}", self.name, self.expression)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = DefineMetricOp::setup("total_revenue", "SUM(amount)", None);
        assert_eq!(op.describe(), "CREATE METRIC: total_revenue = SUM(amount)");
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
