use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetMinVersionOp {
    pub version: String,
}

impl SetMinVersionOp {
    pub fn setup(version: &str) -> Self {
        Self {
            version: version.to_string(),
        }
    }
}

impl Operation for SetMinVersionOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        let _ = bundlebase_common::parse_format_version(&self.version);
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        *bundle.min_version.write() = Some(bundlebase_common::parse_format_version(&self.version));
        Ok(())
    }

    fn describe(&self) -> String {
        format!("SET MIN VERSION: {}", self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_min_version_describe() {
        let op = SetMinVersionOp::setup("0.9");
        assert_eq!(op.describe(), "SET MIN VERSION: 0.9");
    }

    #[test]
    fn test_set_min_version_serialization() {
        let op = SetMinVersionOp::setup("0.9");
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert_eq!(serialized, "version: '0.9'\n");
    }
}
