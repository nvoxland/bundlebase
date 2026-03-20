//! DropConnector operation — removes a connector definition, or a specific platform entry.

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
}

impl DropConnectorOp {
    /// Resolve a connector name (and optional platform) to entry IDs, creating the operation.
    ///
    /// This is the primary constructor — call it at command execution time when
    /// the bundle state is available for name→ID resolution.
    pub fn setup(
        connector_name: &str,
        platform: Option<&crate::platform::Platform>,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        let registry = builder.bundle().connector_registry();
        let registry_guard = registry.read();
        let matching: Vec<&crate::bundle::connector_entry::ConnectorEntry> = registry_guard
            .entries()
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
        Ok(Self { ids })
    }
}

#[async_trait]
impl Operation for DropConnectorOp {
    fn describe(&self) -> String {
        let id_strs: Vec<String> = self.ids.iter().map(|id| id.to_string()).collect();
        format!("DROP CONNECTOR: {}", id_strs.join(", "))
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify at least one of the target IDs still exists
        let registry = bundle.connector_registry();
        let registry_guard = registry.read();
        let found = self.ids.iter().any(|id| registry_guard.entries().iter().any(|e| e.id == *id));
        if !found {
            return Err("Connector entries not found. Use IMPORT CONNECTOR first.".into());
        }
        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        // Look up the connector name before removing entries
        let name = {
            let registry = bundle.connector_registry();
            let reg = registry.read();
            reg.entries().iter()
                .find(|e| self.ids.contains(&e.id))
                .map(|e| e.name.to_string())
        };

        // Remove entries by ID
        bundle.connector_registry().write().remove_entries_by_ids(&self.ids);

        // Remove sources referencing this connector (only if all entries for the name are gone)
        if let Some(name) = name {
            if !bundle.connector_registry().read().has_entry(&name) {
                bundle.sources.write().retain(|_, source| source.connector() != name);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::connector_entry::ConnectorEntry;
    use crate::platform::Platform;
    use crate::udf::UdfRuntime;
    use crate::NamespacedName;

    #[test]
    fn test_describe() {
        let id = ObjectId::generate();
        let op = DropConnectorOp {
            ids: vec![id],
        };
        assert_eq!(op.describe(), format!("DROP CONNECTOR: {}", id));
    }

    #[test]
    fn test_describe_multiple_ids() {
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        let op = DropConnectorOp {
            ids: vec![id1, id2],
        };
        assert_eq!(op.describe(), format!("DROP CONNECTOR: {}, {}", id1, id2));
    }

    #[test]
    fn test_serialization() {
        let op = DropConnectorOp {
            ids: vec![ObjectId::generate()],
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
        };
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_check_connector_defined() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle.connector_registry().write().add_entry(ConnectorEntry {
            id,
            name: NamespacedName::new("acme", "weather"),
            from: UdfRuntime::parse_from("ffi::test").unwrap(),
            platform: Platform::any(),
            temporary: false,
        });

        let op = DropConnectorOp {
            ids: vec![id],
        };
        let result = op.check(&bundle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_apply_removes_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle.connector_registry().write().add_entry(ConnectorEntry {
            id,
            name: NamespacedName::new("acme", "weather"),
            from: UdfRuntime::parse_from("ffi::test").unwrap(),
            platform: Platform::any(),
            temporary: false,
        });

        let op = DropConnectorOp {
            ids: vec![id],
        };
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.connector_registry().read().has_entry("acme.weather"));
    }

    #[tokio::test]
    async fn test_apply_removes_all_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        bundle.connector_registry().write().add_entry(ConnectorEntry {
            id: id1,
            name: NamespacedName::new("acme", "weather"),
            from: UdfRuntime::parse_from("ffi::test1").unwrap(),
            platform: Platform::any(),
            temporary: false,
        });
        bundle.connector_registry().write().add_entry(ConnectorEntry {
            id: id2,
            name: NamespacedName::new("acme", "weather"),
            from: UdfRuntime::parse_from("ffi::test2").unwrap(),
            platform: "linux/amd64".parse().unwrap(),
            temporary: false,
        });

        let op = DropConnectorOp {
            ids: vec![id1, id2],
        };
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.connector_registry().read().has_entry("acme.weather"));
    }

    #[tokio::test]
    async fn test_apply_removes_platform_specific() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        bundle.connector_registry().write().add_entry(ConnectorEntry {
            id: id1,
            name: NamespacedName::new("acme", "weather"),
            from: UdfRuntime::parse_from("ffi::wildcard").unwrap(),
            platform: Platform::any(),
            temporary: false,
        });
        bundle.connector_registry().write().add_entry(ConnectorEntry {
            id: id2,
            name: NamespacedName::new("acme", "weather"),
            from: UdfRuntime::parse_from("ffi::linux-specific").unwrap(),
            platform: "linux/amd64".parse().unwrap(),
            temporary: false,
        });

        let op = DropConnectorOp {
            ids: vec![id2],
        };
        op.apply(&bundle).await.expect("apply");

        // Wildcard entry should remain
        let resolved = bundle.connector_registry().read().resolve_entry("acme.weather").expect("should resolve");
        assert_eq!(resolved.from.to_entrypoint_string(), "wildcard");
    }
}
