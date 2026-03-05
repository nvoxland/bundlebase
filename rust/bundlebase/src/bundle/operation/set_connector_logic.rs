//! SetConnectorLogic operation — binds a platform-specific implementation to a defined connector.

use crate::bundle::operation::Operation;
use crate::bundle::connector_definition::ConnectorLogicEntry;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that sets the implementation logic for a defined connector on a specific platform.
///
/// Always persisted — `SET CONNECTOR LOGIC` always creates this operation.
/// For runtime-only logic (e.g., Python in-process), use `SET TEMPORARY SOURCE LOGIC` instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetConnectorLogicOp {
    /// Full dotted connector name (e.g., "acme.datasources.weather")
    pub source_name: String,
    /// Source type: "lib", "java", "docker", or "ipc"
    pub source_type: String,
    /// Logic string (e.g., path to shared library or binary)
    pub logic: String,
    /// Platform pattern in Docker-style os/arch (e.g., "linux/amd64", "*/*")
    pub platform: String,
}

impl SetConnectorLogicOp {
    pub fn new(source_name: String, source_type: String, logic: String, platform: String) -> Self {
        Self {
            source_name,
            source_type,
            logic,
            platform,
        }
    }
}

#[async_trait]
impl Operation for SetConnectorLogicOp {
    fn describe(&self) -> String {
        format!(
            "SET CONNECTOR LOGIC {} (type={}, platform={})",
            self.source_name, self.source_type, self.platform
        )
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

        // Validate source_type and reject python (cannot be bundled)
        use crate::bundle::connector_definition::{resolve_registry_type, VALID_SOURCE_TYPES};
        resolve_registry_type(&self.source_type)?;
        if self.source_type == "python" {
            return Err(
                "python type cannot be bundled. Use SET TEMPORARY CONNECTOR LOGIC instead.".into(),
            );
        }

        // Validate platform format (must contain '/')
        if !self.platform.contains('/') {
            return Err(format!(
                "Invalid platform '{}'. Must be in os/arch format (e.g., 'linux/amd64', '*/*').",
                self.platform
            )
            .into());
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let entry = ConnectorLogicEntry {
            source_type: self.source_type.clone(),
            logic: self.logic.clone(),
            platform: self.platform.clone(),
        };
        bundle
            .add_connector_logic(&self.source_name, entry)
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::connector_definition::ConnectorDefinition;

    #[test]
    fn test_describe() {
        let op = SetConnectorLogicOp::new(
            "acme.weather".to_string(),
            "lib".to_string(),
            "./libweather.so".to_string(),
            "*/*".to_string(),
        );
        assert_eq!(
            op.describe(),
            "SET CONNECTOR LOGIC acme.weather (type=lib, platform=*/*)"
        );
    }

    #[test]
    fn test_serialization() {
        let op = SetConnectorLogicOp::new(
            "acme.weather".to_string(),
            "ipc".to_string(),
            "./weather-linux".to_string(),
            "linux/amd64".to_string(),
        );
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: SetConnectorLogicOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_source_not_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = SetConnectorLogicOp::new(
            "acme.weather".to_string(),
            "lib".to_string(),
            "./libweather.so".to_string(),
            "*/*".to_string(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not defined"));
    }

    #[tokio::test]
    async fn test_check_invalid_source_type() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_connector_definition(ConnectorDefinition::new("acme.weather".to_string()));

        let op = SetConnectorLogicOp::new(
            "acme.weather".to_string(),
            "invalid".to_string(),
            "test".to_string(),
            "*/*".to_string(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid source type"));
    }

    #[tokio::test]
    async fn test_check_python_type_rejected() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_connector_definition(ConnectorDefinition::new("acme.weather".to_string()));

        let op = SetConnectorLogicOp::new(
            "acme.weather".to_string(),
            "python".to_string(),
            "mod:Class".to_string(),
            "*/*".to_string(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("python type cannot be bundled"));
    }

    #[tokio::test]
    async fn test_check_invalid_platform() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_connector_definition(ConnectorDefinition::new("acme.weather".to_string()));

        let op = SetConnectorLogicOp::new(
            "acme.weather".to_string(),
            "lib".to_string(),
            "test".to_string(),
            "invalid".to_string(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid platform"));
    }
}
