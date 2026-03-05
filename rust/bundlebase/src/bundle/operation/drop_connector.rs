//! DropConnector operation — removes a connector definition and all its logic.

use crate::bundle::operation::Operation;
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that removes a connector definition and all associated logic and sources.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropConnectorOp {
    /// Full dotted connector name (e.g., "acme.datasources.weather")
    pub source_name: String,
}

impl DropConnectorOp {
    pub fn new(source_name: String) -> Self {
        Self { source_name }
    }
}

#[async_trait]
impl Operation for DropConnectorOp {
    fn describe(&self) -> String {
        format!("DROP CONNECTOR {}", self.source_name)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        if bundle.get_connector_definition(&self.source_name).is_none() {
            return Err(format!(
                "Connector '{}' is not defined. Use CREATE CONNECTOR first.",
                self.source_name
            )
            .into());
        }
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle
            .remove_connector_definition(&self.source_name)
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::connector_definition::{ConnectorDefinition, ConnectorLogicEntry};

    #[test]
    fn test_describe() {
        let op = DropConnectorOp::new("acme.weather".to_string());
        assert_eq!(op.describe(), "DROP CONNECTOR acme.weather");
    }

    #[test]
    fn test_serialization() {
        let op = DropConnectorOp::new("acme.weather".to_string());
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: DropConnectorOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_source_not_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = DropConnectorOp::new("acme.weather".to_string());
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not defined"));
    }

    #[tokio::test]
    async fn test_check_source_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_connector_definition(ConnectorDefinition::new("acme.weather".to_string()));

        let op = DropConnectorOp::new("acme.weather".to_string());
        let result = op.check(&bundle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_apply_removes_definition() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_connector_definition(ConnectorDefinition::new("acme.weather".to_string()));
        bundle
            .add_connector_logic(
                "acme.weather",
                ConnectorLogicEntry {
                    source_type: "lib".to_string(),
                    logic: "test".to_string(),
                    platform: "*/*".to_string(),
                },
            )
            .expect("add logic");

        let op = DropConnectorOp::new("acme.weather".to_string());
        op.apply(&bundle).await.expect("apply");

        assert!(bundle.get_connector_definition("acme.weather").is_none());
    }
}
