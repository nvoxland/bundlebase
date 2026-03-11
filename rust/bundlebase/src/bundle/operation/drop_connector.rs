//! DropConnector operation — removes a connector definition, or specific platform logic.

use crate::bundle::connector_definition::Platform;
use crate::bundle::operation::Operation;
use crate::data::ObjectId;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use async_trait::async_trait;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that removes connector entries by their IDs.
///
/// The entry IDs are resolved at command setup time from a connector name
/// (and optional platform filter). The operation itself only stores IDs,
/// making it stable across renames.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DropConnectorOp {
    /// IDs of the connector entries to remove
    pub ids: Vec<ObjectId>,
    /// Connector name (for describe/display only, not used for matching)
    pub connector_name: String,
    /// Optional platform filter (for describe/display only)
    pub platform: Option<Platform>,
}

impl DropConnectorOp {
    /// Resolve a connector name (and optional platform) to entry IDs, creating the operation.
    ///
    /// This is the primary constructor — call it at command execution time when
    /// the bundle state is available for name→ID resolution.
    pub fn setup(
        connector_name: &str,
        platform: Option<&Platform>,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        let entries = builder.bundle().connector_entries.read();
        let matching: Vec<&crate::bundle::connector_definition::ConnectorEntry> = entries
            .iter()
            .filter(|e| {
                if e.name != connector_name {
                    return false;
                }
                if let Some(p) = platform {
                    return &e.platform == p;
                }
                true
            })
            .collect();

        if matching.is_empty() {
            return Err(format!(
                "Connector '{}' is not defined. Use IMPORT CONNECTOR first.",
                connector_name
            )
            .into());
        }

        let ids = matching.iter().map(|e| e.id).collect();
        Ok(Self {
            ids,
            connector_name: connector_name.to_string(),
            platform: platform.cloned(),
        })
    }
}

#[async_trait]
impl Operation for DropConnectorOp {
    fn describe(&self) -> String {
        match &self.platform {
            Some(p) => format!("DROP CONNECTOR {} FOR PLATFORM '{}'", self.connector_name, p),
            None => format!("DROP CONNECTOR {}", self.connector_name),
        }
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify at least one of the target IDs still exists
        let entries = bundle.connector_entries.read();
        let found = self.ids.iter().any(|id| entries.iter().any(|e| e.id == *id));
        if !found {
            return Err(format!(
                "Connector '{}' is not defined. Use IMPORT CONNECTOR first.",
                self.connector_name
            )
            .into());
        }
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Remove entries by ID
        {
            let mut entries = bundle.connector_entries.write();
            entries.retain(|e| !self.ids.contains(&e.id));
        }

        // Also remove any sources that reference the connector name
        if self.platform.is_none() {
            let mut sources = bundle.sources.write();
            sources.retain(|_, source| source.connector() != self.connector_name);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::connector_definition::{ConnectorEntry, Platform, Runner};
    use crate::NamespacedName;

    #[test]
    fn test_describe_without_platform() {
        let op = DropConnectorOp {
            ids: vec![ObjectId::generate()],
            connector_name: "acme.weather".to_string(),
            platform: None,
        };
        assert_eq!(op.describe(), "DROP CONNECTOR acme.weather");
    }

    #[test]
    fn test_describe_with_platform() {
        let op = DropConnectorOp {
            ids: vec![ObjectId::generate()],
            connector_name: "acme.weather".to_string(),
            platform: Some("linux/amd64".parse().unwrap()),
        };
        assert_eq!(
            op.describe(),
            "DROP CONNECTOR acme.weather FOR PLATFORM 'linux/amd64'"
        );
    }

    #[test]
    fn test_serialization() {
        let op = DropConnectorOp {
            ids: vec![ObjectId::generate()],
            connector_name: "acme.weather".to_string(),
            platform: None,
        };
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: DropConnectorOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[test]
    fn test_serialization_with_platform() {
        let op = DropConnectorOp {
            ids: vec![ObjectId::generate()],
            connector_name: "acme.weather".to_string(),
            platform: Some("linux/amd64".parse().unwrap()),
        };
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: DropConnectorOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_connector_not_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = DropConnectorOp {
            ids: vec![ObjectId::generate()],
            connector_name: "acme.weather".to_string(),
            platform: None,
        };
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not defined"));
    }

    #[tokio::test]
    async fn test_check_connector_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle.add_connector_entry(ConnectorEntry {
            id,
            name: NamespacedName::new("acme", "weather"),
            runner: Runner::Lib,
            logic: "test".to_string(),
            platform: Platform::any(),
            temporary: false,
        });

        let op = DropConnectorOp {
            ids: vec![id],
            connector_name: "acme.weather".to_string(),
            platform: None,
        };
        let result = op.check(&bundle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_apply_removes_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle.add_connector_entry(ConnectorEntry {
            id,
            name: NamespacedName::new("acme", "weather"),
            runner: Runner::Lib,
            logic: "test".to_string(),
            platform: Platform::any(),
            temporary: false,
        });

        let op = DropConnectorOp {
            ids: vec![id],
            connector_name: "acme.weather".to_string(),
            platform: None,
        };
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.has_connector_entry("acme.weather"));
    }

    #[tokio::test]
    async fn test_apply_removes_all_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        bundle.add_connector_entry(ConnectorEntry {
            id: id1,
            name: NamespacedName::new("acme", "weather"),
            runner: Runner::Lib,
            logic: "test1".to_string(),
            platform: Platform::any(),
            temporary: false,
        });
        bundle.add_connector_entry(ConnectorEntry {
            id: id2,
            name: NamespacedName::new("acme", "weather"),
            runner: Runner::Lib,
            logic: "test2".to_string(),
            platform: "linux/amd64".parse().unwrap(),
            temporary: false,
        });

        // Drop entire connector
        let op = DropConnectorOp {
            ids: vec![id1, id2],
            connector_name: "acme.weather".to_string(),
            platform: None,
        };
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.has_connector_entry("acme.weather"));
    }

    #[tokio::test]
    async fn test_apply_removes_platform_specific() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        bundle.add_connector_entry(ConnectorEntry {
            id: id1,
            name: NamespacedName::new("acme", "weather"),
            runner: Runner::Lib,
            logic: "wildcard".to_string(),
            platform: Platform::any(),
            temporary: false,
        });
        bundle.add_connector_entry(ConnectorEntry {
            id: id2,
            name: NamespacedName::new("acme", "weather"),
            runner: Runner::Lib,
            logic: "linux-specific".to_string(),
            platform: "linux/amd64".parse().unwrap(),
            temporary: false,
        });

        let op = DropConnectorOp {
            ids: vec![id2],
            connector_name: "acme.weather".to_string(),
            platform: Some("linux/amd64".parse().unwrap()),
        };
        op.apply(&bundle).await.expect("apply");

        // Wildcard entry should remain
        let resolved = bundle.resolve_connector("acme.weather").expect("should resolve");
        assert_eq!(resolved.logic, "wildcard");
    }
}
