//! DropConnectorLogic operation — removes logic entries from a defined connector.

use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that removes implementation logic from a defined connector.
///
/// If `platform` is None, all logic entries are removed.
/// If `platform` is Some, only entries matching that platform are removed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropConnectorLogicOp {
    /// Full dotted connector name (e.g., "acme.datasources.weather")
    pub source_name: String,
    /// Optional platform filter (e.g., "linux/amd64"). None means drop all.
    pub platform: Option<String>,
}

impl DropConnectorLogicOp {
    pub fn new(source_name: String, platform: Option<String>) -> Self {
        Self {
            source_name,
            platform,
        }
    }
}

#[async_trait]
impl Operation for DropConnectorLogicOp {
    fn describe(&self) -> String {
        match &self.platform {
            Some(p) => format!("DROP CONNECTOR LOGIC {} FOR PLATFORM '{}'", self.source_name, p),
            None => format!("DROP CONNECTOR LOGIC {}", self.source_name),
        }
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Connector must be defined
        if bundle.get_connector_definition(&self.source_name).is_none() {
            return Err(format!(
                "Connector '{}' is not defined. Use CREATE CONNECTOR first.",
                self.source_name
            )
            .into());
        }

        // Validate platform format if provided
        if let Some(ref platform) = self.platform {
            if !platform.contains('/') {
                return Err(format!(
                    "Invalid platform '{}'. Must be in os/arch format (e.g., 'linux/amd64', '*/*').",
                    platform
                )
                .into());
            }
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle
            .remove_connector_logic(&self.source_name, self.platform.as_deref())
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::connector_definition::{ConnectorDefinition, ConnectorLogicEntry};

    #[test]
    fn test_describe_without_platform() {
        let op = DropConnectorLogicOp::new("acme.weather".to_string(), None);
        assert_eq!(op.describe(), "DROP CONNECTOR LOGIC acme.weather");
    }

    #[test]
    fn test_describe_with_platform() {
        let op = DropConnectorLogicOp::new(
            "acme.weather".to_string(),
            Some("linux/amd64".to_string()),
        );
        assert_eq!(
            op.describe(),
            "DROP CONNECTOR LOGIC acme.weather FOR PLATFORM 'linux/amd64'"
        );
    }

    #[test]
    fn test_serialization() {
        let op = DropConnectorLogicOp::new(
            "acme.weather".to_string(),
            Some("linux/amd64".to_string()),
        );
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: DropConnectorLogicOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[test]
    fn test_serialization_no_platform() {
        let op = DropConnectorLogicOp::new("acme.weather".to_string(), None);
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: DropConnectorLogicOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_source_not_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = DropConnectorLogicOp::new("acme.weather".to_string(), None);
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not defined"));
    }

    #[tokio::test]
    async fn test_check_invalid_platform() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_connector_definition(ConnectorDefinition::new("acme.weather".to_string()));

        let op = DropConnectorLogicOp::new(
            "acme.weather".to_string(),
            Some("invalid".to_string()),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid platform"));
    }

    #[tokio::test]
    async fn test_apply_removes_all_logic() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_connector_definition(ConnectorDefinition::new("acme.weather".to_string()));
        bundle
            .add_connector_logic(
                "acme.weather",
                ConnectorLogicEntry {
                    source_type: "lib".to_string(),
                    logic: "test1".to_string(),
                    platform: "*/*".to_string(),
                },
            )
            .expect("add logic");
        bundle
            .add_connector_logic(
                "acme.weather",
                ConnectorLogicEntry {
                    source_type: "lib".to_string(),
                    logic: "test2".to_string(),
                    platform: "linux/amd64".to_string(),
                },
            )
            .expect("add logic");

        let op = DropConnectorLogicOp::new("acme.weather".to_string(), None);
        op.apply(&bundle).await.expect("apply");

        // Should have no logic entries left
        let def = bundle.get_connector_definition("acme.weather").expect("def");
        assert!(def.resolve_logic().is_err());
    }

    #[tokio::test]
    async fn test_apply_removes_platform_specific() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_connector_definition(ConnectorDefinition::new("acme.weather".to_string()));
        bundle
            .add_connector_logic(
                "acme.weather",
                ConnectorLogicEntry {
                    source_type: "lib".to_string(),
                    logic: "wildcard".to_string(),
                    platform: "*/*".to_string(),
                },
            )
            .expect("add logic");
        bundle
            .add_connector_logic(
                "acme.weather",
                ConnectorLogicEntry {
                    source_type: "lib".to_string(),
                    logic: "linux-specific".to_string(),
                    platform: "linux/amd64".to_string(),
                },
            )
            .expect("add logic");

        let op = DropConnectorLogicOp::new(
            "acme.weather".to_string(),
            Some("linux/amd64".to_string()),
        );
        op.apply(&bundle).await.expect("apply");

        // Wildcard entry should remain
        let def = bundle.get_connector_definition("acme.weather").expect("def");
        let resolved = def.resolve_logic().expect("should resolve");
        assert_eq!(resolved.logic, "wildcard");
    }
}
