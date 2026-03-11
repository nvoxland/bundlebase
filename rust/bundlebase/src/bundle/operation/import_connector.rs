//! ImportConnector operation — registers a named connector definition with logic.

use crate::bundle::operation::Operation;
use crate::bundle::connector_definition::{parse_connector_name, ConnectorEntry, Platform, Runner};
use crate::NamespacedName;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that defines a named connector and sets its logic.
///
/// Loads the connector if it doesn't exist, then adds/replaces logic for the given platform.
/// Always persisted — for runtime-only logic, use `import_temp_connector` instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportConnectorOp {
    /// Full dotted connector name (e.g., "acme.weather")
    pub name: String,
    /// Runner type
    pub runner: Runner,
    /// Logic string (e.g., path to shared library or binary)
    pub logic: String,
    /// Platform pattern in Docker-style os/arch (e.g., "linux/amd64", "*/*")
    pub platform: Platform,
}

impl ImportConnectorOp {
    pub fn new(name: String, runner: Runner, logic: String, platform: Platform) -> Self {
        Self { name, runner, logic, platform }
    }
}

#[async_trait]
impl Operation for ImportConnectorOp {
    fn describe(&self) -> String {
        format!(
            "IMPORT CONNECTOR {} (runner={}, platform={})",
            self.name, self.runner, self.platform
        )
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Validate name has at least one dot
        parse_connector_name(&self.name)?;

        // Reject python runner (cannot be bundled)
        if self.runner == Runner::Python {
            return Err(
                "python runner cannot be bundled. Use IMPORT TEMP CONNECTOR instead.".into(),
            );
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let namespaced = self.name.parse::<NamespacedName>()
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        bundle.add_connector_entry(ConnectorEntry {
            name: namespaced,
            runner: self.runner,
            logic: self.logic.clone(),
            platform: self.platform.clone(),
            temporary: false,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = ImportConnectorOp::new(
            "acme.weather".to_string(),
            Runner::Ipc,
            "./weather".to_string(),
            Platform::any(),
        );
        assert_eq!(
            op.describe(),
            "IMPORT CONNECTOR acme.weather (runner=ipc, platform=*/*)"
        );
    }

    #[test]
    fn test_serialization() {
        let op = ImportConnectorOp::new(
            "acme.weather".to_string(),
            Runner::Ipc,
            "./weather-linux".to_string(),
            "linux/amd64".parse().unwrap(),
        );
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        assert!(yaml.contains("linux/amd64"));
        let deser: ImportConnectorOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_no_dot() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = ImportConnectorOp::new(
            "weather".to_string(),
            Runner::Ipc,
            "./test".to_string(),
            Platform::any(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be in format 'namespace.name'"));
    }

    #[tokio::test]
    async fn test_check_python_rejected() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = ImportConnectorOp::new(
            "acme.weather".to_string(),
            Runner::Python,
            "mod:Class".to_string(),
            Platform::any(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("python runner cannot be bundled"));
    }
}
