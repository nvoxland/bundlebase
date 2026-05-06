use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetMaxVersionOp {
    pub version: String,
}

impl SetMaxVersionOp {
    pub fn setup(version: &str) -> Self {
        // Store only major.minor — patch is irrelevant to format compatibility
        let (major, minor) = bundlebase_common::parse_format_version(version);
        Self {
            version: format!("{}.{}", major, minor),
        }
    }
}

impl Operation for SetMaxVersionOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        let _ = bundlebase_common::parse_format_version(&self.version);
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        *bundle.max_version.write() = Some(bundlebase_common::parse_format_version(&self.version));
        Ok(())
    }

    fn describe(&self) -> String {
        format!("SET MAX VERSION: {}", self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_max_version_describe() {
        let op = SetMaxVersionOp::setup("0.9");
        assert_eq!(op.describe(), "SET MAX VERSION: 0.9");
    }

    #[test]
    fn test_set_max_version_serialization() {
        let op = SetMaxVersionOp::setup("0.9");
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert_eq!(serialized, "version: '0.9'\n");
    }

    #[test]
    fn test_set_max_version_drops_patch() {
        // Patch is normalized away on construction — only major.minor is stored.
        let op = SetMaxVersionOp::setup("1.2.3");
        assert_eq!(op.version, "1.2");
        assert_eq!(op.describe(), "SET MAX VERSION: 1.2");

        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        assert_eq!(serialized, "version: '1.2'\n");
    }
}
