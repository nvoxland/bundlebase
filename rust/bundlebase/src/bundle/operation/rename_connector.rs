//! RenameConnector operation — renames a connector definition.
//!
//! Stores the entry IDs and new name. The IDs are resolved at command setup
//! time from the old connector name. The operation itself only stores IDs
//! (consistent with DropConnectorOp).

use crate::bundle::operation::Operation;
use crate::data::ObjectId;
use crate::namespaced_name::NamespacedName;
use crate::{Bundle, BundleBuilder, BundlebaseError};
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};

/// Operation that renames connector entries and updates sources referencing the old name.
///
/// The entry IDs are resolved at command setup time from a connector name.
/// The operation stores IDs and the new name, making it stable across
/// intermediate renames.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenameConnectorOp {
    /// IDs of the connector entries to rename
    pub ids: Vec<ObjectId>,
    /// New connector name (dotted, e.g. "acme.weather_v2")
    pub new_name: String,
}

impl RenameConnectorOp {
    /// Resolve a connector name to entry IDs and validate the rename.
    ///
    /// Checks that the old name exists, the new name is a valid namespaced name,
    /// and the new name doesn't already exist.
    pub fn setup(
        old_name: &str,
        new_name: &str,
        builder: &BundleBuilder,
    ) -> Result<Self, BundlebaseError> {
        // Validate new_name format
        let _new_namespaced = NamespacedName::parse(new_name, "Connector")?;

        let registry = builder.bundle().connector_registry();
        let registry_guard = registry.read();

        // Find entries matching old name
        let matching: Vec<_> = registry_guard
            .entries()
            .iter()
            .filter(|e| e.name == old_name)
            .collect();

        if matching.is_empty() {
            return Err(format!(
                "Connector '{}' is not defined. Use IMPORT CONNECTOR first.",
                old_name
            )
            .into());
        }

        // Check new name doesn't already exist
        if registry_guard.has_entry(new_name) {
            return Err(format!(
                "Connector '{}' already exists. Drop it first or choose a different name.",
                new_name
            )
            .into());
        }

        let ids = matching.iter().map(|e| e.id).collect();
        Ok(Self {
            ids,
            new_name: new_name.to_string(),
        })
    }
}

impl Operation for RenameConnectorOp {
    fn describe(&self) -> String {
        let id_strs: Vec<String> = self.ids.iter().map(|id| id.to_string()).collect();
        format!("RENAME CONNECTOR: {} to '{}'", id_strs.join(", "), self.new_name)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        let registry = bundle.connector_registry();
        let registry_guard = registry.read();

        // Verify at least one of the target IDs still exists
        let found = self
            .ids
            .iter()
            .any(|id| registry_guard.entries().iter().any(|e| e.id == *id));
        if !found {
            return Err("Connector entries not found. Use IMPORT CONNECTOR first.".into());
        }

        // Check new name doesn't already exist
        if registry_guard.has_entry(&self.new_name) {
            return Err(format!(
                "Connector '{}' already exists. Drop it first or choose a different name.",
                self.new_name
            )
            .into());
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        let new_namespaced = NamespacedName::parse(&self.new_name, "Connector")
            .map_err(|e| DataFusionError::Execution(e.to_string()))?;

        // Look up the old name before renaming
        let old_name = {
            let registry = bundle.connector_registry();
            let reg = registry.read();
            reg.entries()
                .iter()
                .find(|e| self.ids.contains(&e.id))
                .map(|e| e.name.to_string())
        };

        // Rename entries in the connector registry
        bundle
            .connector_registry()
            .write()
            .rename_entries(&self.ids, &new_namespaced);

        // Update sources referencing the old connector name
        if let Some(old_name) = old_name {
            let sources = bundle.sources.read();
            for (_, source) in sources.iter() {
                if source.connector() == old_name {
                    source.set_connector_name(self.new_name.clone());
                }
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
        let op = RenameConnectorOp {
            ids: vec![id],
            new_name: "acme.weather_v2".to_string(),
        };
        assert_eq!(
            op.describe(),
            format!("RENAME CONNECTOR: {} to 'acme.weather_v2'", id)
        );
    }

    #[test]
    fn test_describe_multiple_ids() {
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        let op = RenameConnectorOp {
            ids: vec![id1, id2],
            new_name: "acme.weather_v2".to_string(),
        };
        assert_eq!(
            op.describe(),
            format!("RENAME CONNECTOR: {}, {} to 'acme.weather_v2'", id1, id2)
        );
    }

    #[test]
    fn test_serialization() {
        let op = RenameConnectorOp {
            ids: vec![ObjectId::generate()],
            new_name: "acme.weather_v2".to_string(),
        };
        let yaml = serde_yaml_ng::to_string(&op).expect("serialize");
        let deser: RenameConnectorOp = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(deser, op);
    }

    #[tokio::test]
    async fn test_check_not_found() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = RenameConnectorOp {
            ids: vec![ObjectId::generate()],
            new_name: "acme.weather_v2".to_string(),
        };
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_check_found() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle
            .connector_registry()
            .write()
            .add_entry(ConnectorEntry {
                id,
                name: NamespacedName::new("acme", "weather"),
                from: UdfRuntime::parse_from("ffi::test").unwrap(),
                platform: Platform::any(),
                temporary: false,
            });

        let op = RenameConnectorOp {
            ids: vec![id],
            new_name: "acme.weather_v2".to_string(),
        };
        let result = op.check(&bundle).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_check_new_name_already_exists() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle
            .connector_registry()
            .write()
            .add_entry(ConnectorEntry {
                id,
                name: NamespacedName::new("acme", "weather"),
                from: UdfRuntime::parse_from("ffi::test").unwrap(),
                platform: Platform::any(),
                temporary: false,
            });
        bundle
            .connector_registry()
            .write()
            .add_entry(ConnectorEntry {
                id: ObjectId::generate(),
                name: NamespacedName::new("acme", "weather_v2"),
                from: UdfRuntime::parse_from("ffi::test2").unwrap(),
                platform: Platform::any(),
                temporary: false,
            });

        let op = RenameConnectorOp {
            ids: vec![id],
            new_name: "acme.weather_v2".to_string(),
        };
        let result = op.check(&bundle).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_apply_renames_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id = ObjectId::generate();
        bundle
            .connector_registry()
            .write()
            .add_entry(ConnectorEntry {
                id,
                name: NamespacedName::new("acme", "weather"),
                from: UdfRuntime::parse_from("ffi::test").unwrap(),
                platform: Platform::any(),
                temporary: false,
            });

        let op = RenameConnectorOp {
            ids: vec![id],
            new_name: "acme.weather_v2".to_string(),
        };
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.connector_registry().read().has_entry("acme.weather"));
        assert!(bundle
            .connector_registry()
            .read()
            .has_entry("acme.weather_v2"));
    }

    #[tokio::test]
    async fn test_apply_renames_all_matching_entries() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let id1 = ObjectId::generate();
        let id2 = ObjectId::generate();
        bundle
            .connector_registry()
            .write()
            .add_entry(ConnectorEntry {
                id: id1,
                name: NamespacedName::new("acme", "weather"),
                from: UdfRuntime::parse_from("ffi::test1").unwrap(),
                platform: Platform::any(),
                temporary: false,
            });
        bundle
            .connector_registry()
            .write()
            .add_entry(ConnectorEntry {
                id: id2,
                name: NamespacedName::new("acme", "weather"),
                from: UdfRuntime::parse_from("ffi::test2").unwrap(),
                platform: "linux/amd64".parse().unwrap(),
                temporary: false,
            });

        let op = RenameConnectorOp {
            ids: vec![id1, id2],
            new_name: "acme.weather_v2".to_string(),
        };
        op.apply(&bundle).await.expect("apply");

        assert!(!bundle.connector_registry().read().has_entry("acme.weather"));
        assert!(bundle
            .connector_registry()
            .read()
            .has_entry("acme.weather_v2"));
        // Both entries should be renamed
        let entries: Vec<_> = bundle
            .connector_registry()
            .read()
            .entries()
            .iter()
            .filter(|e| e.name == "acme.weather_v2")
            .cloned()
            .collect();
        assert_eq!(entries.len(), 2);
    }
}
