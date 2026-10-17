use super::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// A property-level semantic metadata operation, resolved from bundle history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetDatasetDescriptionOp {
    pub description: Option<String>,
}

impl SetDatasetDescriptionOp {
    pub fn setup(description: Option<&str>) -> Self {
        Self {
            description: description.map(str::to_string),
        }
    }
}

impl Operation for SetDatasetDescriptionOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, _bundle: &Bundle) -> Result<(), DataFusionError> {
        Ok(())
    }

    fn describe(&self) -> String {
        format!("SET DATASET DESCRIPTION: {:?}", self.description)
    }
}
