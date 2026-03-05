//! CreateConnector operation — registers a named connector definition.

use crate::bundle::operation::Operation;
use crate::bundle::connector_definition::{parse_connector_name, ConnectorDefinition};
use crate::{Bundle, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that defines a named connector (e.g., "acme.datasources.weather").
///
/// Must be called before `SetConnectorLogicOp` or `CreateSourceOp` with a dotted name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateConnectorOp {
    /// Full dotted connector name (e.g., "acme.datasources.weather")
    pub name: String,
}

impl CreateConnectorOp {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

#[async_trait]
impl Operation for CreateConnectorOp {
    fn describe(&self) -> String {
        format!("CREATE CONNECTOR {}", self.name)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Validate name has at least one dot
        parse_connector_name(&self.name)?;

        // Check not already defined
        if bundle.get_connector_definition(&self.name).is_some() {
            return Err(format!("Connector '{}' is already defined", self.name).into());
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle.add_connector_definition(ConnectorDefinition::new(self.name.clone()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_describe() {
        let op = CreateConnectorOp::new("acme.weather".to_string());
        assert_eq!(op.describe(), "CREATE CONNECTOR acme.weather");
    }

    #[test]
    fn test_serialization() {
        let op = CreateConnectorOp::new("acme.datasources.weather".to_string());
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: CreateConnectorOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_duplicate_name() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_connector_definition(ConnectorDefinition::new("acme.weather".to_string()));

        let op = CreateConnectorOp::new("acme.weather".to_string());
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already defined"));
    }

    #[tokio::test]
    async fn test_check_no_dot() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = CreateConnectorOp::new("weather".to_string());
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must contain at least one dot"));
    }
}
