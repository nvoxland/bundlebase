use crate::bundle::operation::Operation;
use crate::bundle_config::Scope;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation to create a named scope alias (name -> normalized Scope mapping).
///
/// Scope aliases are runtime aliases that allow `SAVE CONFIG ... FOR SCOPE <name>`
/// to resolve the scope name to a normalized Scope. The resolved scope is always stored
/// in `SaveConfigOp`, never the scope name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateScopeAliasOp {
    /// Scope name (e.g., "prod", "staging")
    pub name: String,

    /// Normalized scope this alias maps to (e.g., "/s3/my-bucket")
    pub scope: Scope,
}

impl CreateScopeAliasOp {
    /// Create a new CreateScopeAliasOp
    pub fn setup(name: &str, scope: &Scope) -> Self {
        Self {
            name: name.to_string(),
            scope: scope.clone(),
        }
    }
}

#[async_trait]
impl Operation for CreateScopeAliasOp {
    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        if self.name.is_empty() {
            return Err("Scope alias name cannot be empty".into());
        }
        if self.scope.is_global() {
            return Err("Scope alias must map to a specific scope, not global '/'".into());
        }
        Ok(())
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle.config().add_scope_alias(&self.name, &self.scope);
        // Refresh data_dir so env vars with named scopes take effect
        bundle
            .refresh_data_dir()
            .map_err(DataFusionError::External)?;
        Ok(())
    }

    fn describe(&self) -> String {
        format!("CREATE SCOPE ALIAS: {} = {}", self.name, self.scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup() {
        let op = CreateScopeAliasOp::setup("prod", &Scope::from_url("s3://my-bucket/"));
        assert_eq!(op.name, "prod");
        assert_eq!(op.scope, Scope::from_url("s3://my-bucket/"));
    }

    #[test]
    fn test_describe() {
        let op = CreateScopeAliasOp::setup("prod", &Scope::from_url("s3://my-bucket/"));
        assert_eq!(
            op.describe(),
            "CREATE SCOPE ALIAS: prod = /s3/my-bucket"
        );
    }

    #[test]
    fn test_serialization_roundtrip() {
        let op = CreateScopeAliasOp::setup("staging", &Scope::from_url("gs://staging-bucket/data/"));
        let serialized = serde_yaml_ng::to_string(&op).expect("Failed to serialize");
        let deserialized: CreateScopeAliasOp =
            serde_yaml_ng::from_str(&serialized).expect("Failed to deserialize");
        assert_eq!(deserialized, op);
    }

    #[tokio::test]
    async fn test_last_scope_wins_same_name() {
        let builder =
            crate::BundleBuilder::create("memory:///test_last_scope_wins", None)
                .await
                .expect("Failed to create test bundle");

        let op1 = CreateScopeAliasOp::setup("prod", &Scope::from_url("s3://old-bucket/"));
        op1.apply(builder.bundle()).await.expect("apply op1");

        let op2 = CreateScopeAliasOp::setup("prod", &Scope::from_url("s3://new-bucket/"));
        op2.apply(builder.bundle()).await.expect("apply op2");

        let scopes = builder.bundle().scope_aliases();
        assert_eq!(
            scopes.get("prod"),
            Some(&Scope::from_url("s3://new-bucket/")),
            "last CreateScopeAliasOp should win"
        );
    }

    #[test]
    fn test_deserialization() {
        let yaml = r#"
name: prod
scope: /s3/my-bucket
"#;
        let op: CreateScopeAliasOp =
            serde_yaml_ng::from_str(yaml).expect("Failed to deserialize");
        assert_eq!(op.name, "prod");
        assert_eq!(op.scope, Scope::new("/s3/my-bucket"));
    }
}
