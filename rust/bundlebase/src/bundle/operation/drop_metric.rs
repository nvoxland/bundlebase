use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// Remove a previously-defined named metric from the bundle. Metadata-only: it
/// records that a metric name should no longer resolve, and is replayed
/// alongside `DefineMetric` operations so the metric set reflects the latest
/// define/drop for each name (see `BundleFacade::metrics`). Dropping a name
/// that was never defined is a harmless no-op.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropMetricOp {
    pub name: String,
}

impl DropMetricOp {
    pub fn setup(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

impl Operation for DropMetricOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        if self.name.trim().is_empty() {
            return Err("Metric name cannot be empty".into());
        }
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        // Metadata-only; metrics are resolved from the operation list.
        Ok(())
    }

    fn describe(&self) -> String {
        format!("DROP METRIC: {}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_round_trip() {
        let op = DropMetricOp::setup("total_revenue");
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let back: DropMetricOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(op, back);
    }

    #[test]
    fn test_describe() {
        let op = DropMetricOp::setup("aov");
        assert_eq!(op.describe(), "DROP METRIC: aov");
    }
}
