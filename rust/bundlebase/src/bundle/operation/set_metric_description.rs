use super::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// A property-level semantic metadata operation, resolved from bundle history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetMetricDescriptionOp {
    pub name: String,
    pub description: Option<String>,
}

impl SetMetricDescriptionOp {
    pub fn setup(name: &str, description: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            description: description.map(str::to_string),
        }
    }
}

impl Operation for SetMetricDescriptionOp {
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        super::semantic_validation::check_metric(bundle, &self.name)
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    fn describe(&self) -> String {
        format!(
            "SET METRIC DESCRIPTION: {} = {:?}",
            self.name, self.description
        )
    }
}
