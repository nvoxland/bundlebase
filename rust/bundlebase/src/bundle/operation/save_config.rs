use crate::bundle::operation::Operation;
use crate::bundle_config::{ConfigKey, Scope};
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation to save a configuration key-value pair in the container.
///
/// Config must be set for a named scope (e.g., "s3", "s3/bucket", "system").
/// Config stored via this operation has the lowest priority in the config resolution:
/// 1. Explicit config passed to create()/open() (highest)
/// 2. Environment variables
/// 3. Config from SaveConfigOp operations (lowest)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveConfigOp {
    /// Named scope (e.g., "s3", "s3/bucket", "system").
    pub scope: Scope,

    /// Configuration key (e.g., "region", "access_key_id")
    pub key: String,

    /// Configuration value
    pub value: String,
}

impl SaveConfigOp {
    /// Create a new SaveConfigOp
    ///
    /// # Arguments
    /// * `key` - Configuration key
    /// * `value` - Configuration value
    /// * `scope` - Normalized scope for this configuration
    pub fn setup(scope: &Scope, key: &str, value: &str) -> Self {
        Self {
            key: key.to_string(),
            value: value.to_string(),
            scope: scope.clone(),
        }
    }
}

#[async_trait]
impl Operation for SaveConfigOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        let specs = crate::all_config_specs();
        ConfigKey::validate_key_scoped(&self.key, &self.scope, &specs)?;
        if ConfigKey::is_key_secure(&self.key, &specs) {
            return Err(format!(
                "Cannot save secure config key '{}'. Use environment variables or pass config at runtime instead.",
                self.key
            )
            .into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle.config().set(
            &self.scope,
            &self.key,
            &self.value,
            crate::bundle_config::ConfigSource::Stored,
        ).map_err(DataFusionError::External)?;

        // Recreate data_dir with updated config
        bundle
            .refresh_data_dir()
            .map_err(DataFusionError::External)?;

        Ok(())
    }

    fn describe(&self) -> String {
        let specs = crate::all_config_specs();
        let display_value = if ConfigKey::is_key_secure(&self.key, &specs) {
            "*****"
        } else {
            &self.value
        };
        format!("SAVE CONFIG [{}]: {} = {}", self.scope, self.key, display_value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup_named_scope_config() {
        let op = SaveConfigOp::setup(&Scope::try_from("s3").unwrap(), "region", "us-west-2");
        assert_eq!(op.key, "region");
        assert_eq!(op.value, "us-west-2");
        assert_eq!(op.scope, Scope::try_from("s3").unwrap());
    }

    #[test]
    fn test_setup_url_specific_config() {
        let op = SaveConfigOp::setup(&Scope::try_from("s3://test").unwrap(), "endpoint", "http://localhost:9000");
        assert_eq!(op.key, "endpoint");
        assert_eq!(op.value, "http://localhost:9000");
        assert_eq!(op.scope, Scope::try_from("s3://test").unwrap());
    }

    #[test]
    fn test_describe_named_scope() {
        let op = SaveConfigOp::setup(&Scope::try_from("s3").unwrap(), "region", "us-west-2");
        assert_eq!(op.describe(), "SAVE CONFIG [s3]: region = us-west-2");
    }

    #[test]
    fn test_describe_url_specific() {
        let op = SaveConfigOp::setup(&Scope::try_from("s3://test/").unwrap(), "endpoint", "http://localhost:9000");
        assert_eq!(
            op.describe(),
            "SAVE CONFIG [s3/test]: endpoint = http://localhost:9000"
        );
    }

    #[test]
    fn test_describe_masks_secure_key() {
        let op =
            SaveConfigOp::setup(&Scope::try_from("s3://bucket/").unwrap(), "secret_access_key", "SUPERSECRET");
        assert_eq!(
            op.describe(),
            "SAVE CONFIG [s3/bucket]: secret_access_key = *****"
        );
    }

    #[test]
    fn test_describe_masks_secure_key_named_scope() {
        let op = SaveConfigOp::setup(&Scope::try_from("s3").unwrap(), "secret_access_key", "SUPERSECRET");
        assert_eq!(op.describe(), "SAVE CONFIG [s3]: secret_access_key = *****");
    }

    #[test]
    fn test_serialization_named_scope() {
        let op = SaveConfigOp::setup(&Scope::try_from("s3").unwrap(), "region", "us-west-2");
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let expected = "scope: s3\nkey: region\nvalue: us-west-2\n";
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_serialization_url_specific() {
        let op = SaveConfigOp::setup(&Scope::try_from("s3://test/").unwrap(), "endpoint", "http://localhost:9000");
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");

        // Deserialize to verify round-trip
        let deserialized: SaveConfigOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[tokio::test]
    async fn test_check_rejects_secure_key() {
        let bundle = crate::BundleBuilder::create("memory:///test_secure_check", None)
            .await
            .expect("Failed to create test bundle");

        let op = SaveConfigOp::setup(&Scope::try_from("s3://bucket/").unwrap(), "secret_access_key", "SECRETVALUE");
        let result = op.check(bundle.bundle()).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Cannot save secure config key"),
            "Expected secure key rejection error, got: {}",
            err_msg
        );

        // Non-secure key should pass check
        let op = SaveConfigOp::setup(&Scope::try_from("s3://bucket/").unwrap(), "region", "us-west-2");
        let result = op.check(bundle.bundle()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_check_rejects_secure_key_named_scope() {
        let bundle = crate::BundleBuilder::create("memory:///test_secure_named", None)
            .await
            .expect("Failed to create test bundle");

        let op = SaveConfigOp::setup(&Scope::try_from("s3").unwrap(), "secret_access_key", "SECRETVALUE");
        let result = op.check(bundle.bundle()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_last_save_config_wins_same_key() {
        let builder = crate::BundleBuilder::create("memory:///test_last_config_wins", None)
            .await
            .expect("Failed to create test bundle");

        let scope = Scope::try_from("s3").unwrap();
        let op1 = SaveConfigOp::setup(&scope, "region", "us-west-1");
        op1.apply(builder.bundle()).await.expect("apply op1");

        let op2 = SaveConfigOp::setup(&scope, "region", "us-east-1");
        op2.apply(builder.bundle()).await.expect("apply op2");

        let config = builder.bundle().config();
        let active = config.values(&[]).unwrap();
        let region = active
            .iter()
            .find(|e| e.key == "region")
            .expect("region entry");
        assert_eq!(region.value, "us-east-1", "last SaveConfigOp should win");
    }

    #[tokio::test]
    async fn test_last_save_config_wins_url_scoped() {
        let builder = crate::BundleBuilder::create("memory:///test_last_config_url", None)
            .await
            .expect("Failed to create test bundle");

        let scope = Scope::try_from("s3://bucket/").unwrap();
        let op1 = SaveConfigOp::setup(&scope, "endpoint", "http://old:9000");
        op1.apply(builder.bundle()).await.expect("apply op1");

        let op2 = SaveConfigOp::setup(&scope, "endpoint", "http://new:9000");
        op2.apply(builder.bundle()).await.expect("apply op2");

        let config = builder.bundle().config();
        let active = config.values(&[]).unwrap();
        let endpoint = active
            .iter()
            .find(|e| e.key == "endpoint" && e.scope == scope)
            .expect("endpoint entry");
        assert_eq!(
            endpoint.value, "http://new:9000",
            "last SaveConfigOp for same key+scope should win"
        );
    }

    #[test]
    fn test_deserialization() {
        let yaml = r#"
key: region
value: us-east-1
scope: s3/my-bucket
"#;
        let op: SaveConfigOp = serde_yaml_ng::from_str(yaml).expect("Failed to deserialize");
        assert_eq!(op.key, "region");
        assert_eq!(op.value, "us-east-1");
        assert_eq!(op.scope, Scope::try_from("s3/my-bucket").unwrap());
    }
}
