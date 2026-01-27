use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetNameOp {
    pub name: String,
}

impl SetNameOp {
    pub fn setup(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Operation for SetNameOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        *bundle.name.write() = Some(self.name.clone());
        Ok(())
    }

    fn describe(&self) -> String {
        format!("SET NAME: {}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_name_describe() {
        let op = SetNameOp::setup("Test Name");
        assert_eq!(op.describe(), "SET NAME: Test Name");
    }

    #[test]
    fn test_set_name_serialization() {
        let op = SetNameOp::setup("Test Name");
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = "name: Test Name\n";
        assert_eq!(serialized, expected);
    }
}
