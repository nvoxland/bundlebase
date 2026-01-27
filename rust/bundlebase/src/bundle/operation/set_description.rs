use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetDescriptionOp {
    pub description: String,
}

impl SetDescriptionOp {
    pub fn setup(description: &str) -> Self {
        Self {
            description: description.to_string(),
        }
    }
}

#[async_trait]
impl Operation for SetDescriptionOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        *bundle.description.write() = Some(self.description.clone());
        Ok(())
    }

    fn describe(&self) -> String {
        format!("SET DESCRIPTION: {}", self.description)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_description_describe() {
        let op = SetDescriptionOp::setup("Test Description");
        assert_eq!(op.describe(), "SET DESCRIPTION: Test Description");
    }

    #[test]
    fn test_set_description_serialization() {
        let op = SetDescriptionOp::setup("Test Description");
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = "description: Test Description\n";
        assert_eq!(serialized, expected);
    }
}
