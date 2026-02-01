use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation to create a named config scope (name -> URL mapping).
///
/// Config scopes are runtime aliases that allow `SET CONFIG ... FOR SCOPE <name>`
/// to resolve the scope name to a URL prefix. The resolved URL is always stored
/// in `SetConfigOp`, never the scope name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateConfigScopeOp {
    /// Scope name (e.g., "prod", "staging")
    pub name: String,

    /// URL prefix this scope maps to (e.g., "s3://my-bucket/")
    pub url: String,
}

impl CreateConfigScopeOp {
    /// Create a new CreateConfigScopeOp
    pub fn setup(name: &str, url: &str) -> Self {
        Self {
            name: name.to_string(),
            url: url.to_string(),
        }
    }
}

#[async_trait]
impl Operation for CreateConfigScopeOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        if self.name.is_empty() {
            return Err("Config scope name cannot be empty".into());
        }
        if !self.url.contains("://") {
            return Err(format!(
                "Config scope URL must contain '://' (got '{}')",
                self.url
            )
            .into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle.add_config_scope(&self.name, &self.url);
        // Recompute config so env vars with named scopes take effect
        bundle
            .recompute_config()
            .map_err(DataFusionError::External)?;
        Ok(())
    }

    fn describe(&self) -> String {
        format!("CREATE CONFIG SCOPE: {} = {}", self.name, self.url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup() {
        let op = CreateConfigScopeOp::setup("prod", "s3://my-bucket/");
        assert_eq!(op.name, "prod");
        assert_eq!(op.url, "s3://my-bucket/");
    }

    #[test]
    fn test_describe() {
        let op = CreateConfigScopeOp::setup("prod", "s3://my-bucket/");
        assert_eq!(
            op.describe(),
            "CREATE CONFIG SCOPE: prod = s3://my-bucket/"
        );
    }

    #[test]
    fn test_serialization_roundtrip() {
        let op = CreateConfigScopeOp::setup("staging", "gs://staging-bucket/data/");
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let deserialized: CreateConfigScopeOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[test]
    fn test_deserialization() {
        let yaml = r#"
name: prod
url: s3://my-bucket/
"#;
        let op: CreateConfigScopeOp =
            serde_yaml_ng::from_str(yaml).expect("Failed to deserialize");
        assert_eq!(op.name, "prod");
        assert_eq!(op.url, "s3://my-bucket/");
    }
}
