//! CreateConnector operation — registers a named connector definition with logic.

use crate::bundle::operation::Operation;
use crate::bundle::connector_definition::{parse_connector_name, ConnectorDefinition, ConnectorLogicEntry, resolve_registry_type};
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that defines a named connector and sets its logic.
///
/// Creates the connector if it doesn't exist, then adds/replaces logic for the given platform.
/// Always persisted — for runtime-only logic, use `create_temporary_connector` instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateConnectorOp {
    /// Full dotted connector name (e.g., "acme.datasources.weather")
    pub name: String,
    /// Runner: "lib", "java", "docker", or "ipc"
    pub runner: String,
    /// Logic string (e.g., path to shared library or binary)
    pub logic: String,
    /// Platform pattern in Docker-style os/arch (e.g., "linux/amd64", "*/*")
    pub platform: String,
}

impl CreateConnectorOp {
    pub fn new(name: String, runner: String, logic: String, platform: String) -> Self {
        Self { name, runner, logic, platform }
    }
}

#[async_trait]
impl Operation for CreateConnectorOp {
    fn describe(&self) -> String {
        format!(
            "CREATE CONNECTOR {} (runner={}, platform={})",
            self.name, self.runner, self.platform
        )
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Validate name has at least one dot
        parse_connector_name(&self.name)?;

        // Validate runner and reject python (cannot be bundled)
        resolve_registry_type(&self.runner)?;
        if self.runner == "python" {
            return Err(
                "python runner cannot be bundled. Use CREATE TEMPORARY CONNECTOR instead.".into(),
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
        // Create connector definition if it doesn't exist
        if bundle.get_connector_definition(&self.name).is_none() {
            bundle.add_connector_definition(ConnectorDefinition::new(self.name.clone()));
        }

        // Add logic entry
        let entry = ConnectorLogicEntry {
            runner: self.runner.clone(),
            logic: self.logic.clone(),
            platform: self.platform.clone(),
        };
        bundle
            .add_connector_logic(&self.name, entry)
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = CreateConnectorOp::new(
            "acme.weather".to_string(),
            "ipc".to_string(),
            "./weather".to_string(),
            "*/*".to_string(),
        );
        assert_eq!(
            op.describe(),
            "CREATE CONNECTOR acme.weather (runner=ipc, platform=*/*)"
        );
    }

    #[test]
    fn test_serialization() {
        let op = CreateConnectorOp::new(
            "acme.datasources.weather".to_string(),
            "ipc".to_string(),
            "./weather-linux".to_string(),
            "linux/amd64".to_string(),
        );
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: CreateConnectorOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_no_dot() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = CreateConnectorOp::new(
            "weather".to_string(),
            "ipc".to_string(),
            "./test".to_string(),
            "*/*".to_string(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must contain at least one dot"));
    }

    #[tokio::test]
    async fn test_check_invalid_runner() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = CreateConnectorOp::new(
            "acme.weather".to_string(),
            "invalid".to_string(),
            "test".to_string(),
            "*/*".to_string(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid runner"));
    }

    #[tokio::test]
    async fn test_check_python_rejected() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = CreateConnectorOp::new(
            "acme.weather".to_string(),
            "python".to_string(),
            "mod:Class".to_string(),
            "*/*".to_string(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("python runner cannot be bundled"));
    }

    #[tokio::test]
    async fn test_check_invalid_platform() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = CreateConnectorOp::new(
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
