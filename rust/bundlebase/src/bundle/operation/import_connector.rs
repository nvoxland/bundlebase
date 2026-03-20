//! ImportConnector operation — registers a named connector definition.

use crate::bundle::operation::Operation;
use crate::bundle::connector_entry::{parse_connector_name, ConnectorEntry};
use crate::platform::Platform;
use crate::udf::UdfRuntime;
use crate::data::ObjectId;
use crate::NamespacedName;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that defines a named connector and sets its entrypoint.
///
/// Loads the connector if it doesn't exist, then adds/replaces the entrypoint for the given platform.
/// Always persisted — for session-only entrypoints, use `import_temp_connector` instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportConnectorOp {
    /// Unique identifier for this connector entry
    pub id: ObjectId,
    /// Full dotted connector name (e.g., "acme.weather")
    pub name: String,
    /// Runtime and entrypoint source (e.g., ipc::./my_connector)
    pub from: UdfRuntime,
    /// Platform pattern in Docker-style os/arch (e.g., "linux/amd64", "*/*")
    pub platform: Platform,
}

impl ImportConnectorOp {
    pub fn new(name: String, from: UdfRuntime, platform: Platform) -> Self {
        Self { id: ObjectId::generate(), name, from, platform }
    }
}

#[async_trait]
impl Operation for ImportConnectorOp {
    fn describe(&self) -> String {
        format!(
            "IMPORT CONNECTOR {} (runtime={}, platform={})",
            self.name, self.from.runtime_name(), self.platform
        )
    }

    async fn check(&self, _bundle: &Bundle) -> Result<(), BundlebaseError> {
        parse_connector_name(&self.name)?;
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let namespaced = self.name.parse::<NamespacedName>()
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        // Warn if overwriting an existing definition
        if bundle.connector_registry().read().has_entry(&self.name) {
            tracing::warn!("Overwriting existing connector definition for '{}'", self.name);
        }

        bundle.connector_registry().write().add_entry(ConnectorEntry {
            id: self.id,
            name: namespaced,
            from: self.from.clone(),
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
            UdfRuntime::parse_from("ipc::./weather").unwrap(),
            Platform::any(),
        );
        assert_eq!(
            op.describe(),
            "IMPORT CONNECTOR acme.weather (runtime=ipc, platform=*/*)"
        );
    }

    #[test]
    fn test_serialization() {
        let op = ImportConnectorOp::new(
            "acme.weather".to_string(),
            UdfRuntime::parse_from("ipc::./weather-linux").unwrap(),
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
            UdfRuntime::parse_from("ipc::./test").unwrap(),
            Platform::any(),
        );
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be in format 'namespace.name'"));
    }

}
