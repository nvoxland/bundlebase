use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation to set configuration key-value pairs in the container
///
/// Config can be set either as defaults (url_prefix = None) or for specific URL prefixes.
/// Config stored via this operation has the lowest priority in the config resolution:
/// 1. Explicit config passed to create()/open() (highest)
/// 2. Environment variables
/// 3. Config from SetConfigOp operations (lowest)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetConfigOp {
    /// Configuration key (e.g., "region", "access_key_id")
    pub key: String,

    /// Configuration value
    pub value: String,

    /// Optional URL prefix for URL-specific config (e.g., "s3://bucket/")
    /// If None, this is a default setting applicable to all URLs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_prefix: Option<String>,
}

impl SetConfigOp {
    /// Create a new SetConfigOp
    ///
    /// # Arguments
    /// * `key` - Configuration key
    /// * `value` - Configuration value
    /// * `url_prefix` - Optional URL prefix for URL-specific configuration
    pub fn setup(key: &str, value: &str, url_prefix: Option<&str>) -> Self {
        Self {
            key: key.to_string(),
            value: value.to_string(),
            url_prefix: url_prefix.map(|s| s.to_string()),
        }
    }
}

#[async_trait]
impl Operation for SetConfigOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        // No validation needed - config values are validated when used
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Add to bundle.stored_config
        bundle
            .stored_config
            .write()
            .set(&self.key, &self.value, self.url_prefix.as_deref());

        // Recompute merged config and recreate data_dir with new config
        bundle
            .recompute_config()
            .map_err(DataFusionError::External)?;

        Ok(())
    }

    fn describe(&self) -> String {
        match &self.url_prefix {
            Some(prefix) => format!("SET CONFIG [{}]: {} = {}", prefix, self.key, self.value),
            None => format!("SET CONFIG: {} = {}", self.key, self.value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup_default_config() {
        let op = SetConfigOp::setup("region", "us-west-2", None);
        assert_eq!(op.key, "region");
        assert_eq!(op.value, "us-west-2");
        assert_eq!(op.url_prefix, None);
    }

    #[test]
    fn test_setup_url_specific_config() {
        let op = SetConfigOp::setup("endpoint", "http://localhost:9000", Some("s3://test/"));
        assert_eq!(op.key, "endpoint");
        assert_eq!(op.value, "http://localhost:9000");
        assert_eq!(op.url_prefix, Some("s3://test/".to_string()));
    }

    #[test]
    fn test_describe_default() {
        let op = SetConfigOp::setup("region", "us-west-2", None);
        assert_eq!(op.describe(), "SET CONFIG: region = us-west-2");
    }

    #[test]
    fn test_describe_url_specific() {
        let op = SetConfigOp::setup("endpoint", "http://localhost:9000", Some("s3://test/"));
        assert_eq!(
            op.describe(),
            "SET CONFIG [s3://test/]: endpoint = http://localhost:9000"
        );
    }

    #[test]
    fn test_serialization_default() {
        let op = SetConfigOp::setup("region", "us-west-2", None);
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = "key: region\nvalue: us-west-2\n";
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_serialization_url_specific() {
        let op = SetConfigOp::setup("endpoint", "http://localhost:9000", Some("s3://test/"));
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");

        // Deserialize to verify round-trip
        let deserialized: SetConfigOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[test]
    fn test_deserialization() {
        let yaml = r#"
key: region
value: us-east-1
urlPrefix: s3://my-bucket/
"#;
        let op: SetConfigOp = serde_yaml_ng::from_str(yaml).expect("Failed to deserialize");
        assert_eq!(op.key, "region");
        assert_eq!(op.value, "us-east-1");
        assert_eq!(op.url_prefix, Some("s3://my-bucket/".to_string()));
    }
}
