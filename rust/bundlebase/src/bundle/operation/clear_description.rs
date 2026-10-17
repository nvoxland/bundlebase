use super::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// Explicitly clear the model description.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClearDescriptionOp {}

impl ClearDescriptionOp {
    pub fn setup() -> Self {
        Self {}
    }
}

impl Operation for ClearDescriptionOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        *bundle.description.write() = None;
        Ok(())
    }

    fn describe(&self) -> String {
        "CLEAR DESCRIPTION".to_string()
    }
}
