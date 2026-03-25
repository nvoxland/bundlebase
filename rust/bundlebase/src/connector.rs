//! Connector system for data discovery and fetching.
//!
//! Connectors define how data is partitioned, discovered, and made available.
//! Each source divides its data into partitions that become blocks in the bundle.
//! For file-based sources, partitions are individual files. For structured sources
//! (databases, APIs), the connector decides how to chunk data into stable
//! partitions so that sync can detect changes at the block level.
//!
//! The trait has four methods:
//! - `signature()` - Name and argument declarations
//! - `discover()` - Partition source data into locations with version info
//! - `data()` - Provide raw data bytes for a location
//! - `stable_url()` - Provide a stable URL for downloading a location
//!
//! ## Reserved args keys
//!
//! Keys in the `args` HashMap prefixed with `_` are reserved for system use.
//! Currently defined:
//! - `_columns` — Comma-separated column names for optional column pushdown.
//!
//! Orchestration (sync mode handling, materialization, file management)
//! lives in `source::fetch::orchestrate_fetch()`.

// Re-export the connector plugin module from the connector crate
pub use bundlebase_connector::plugin;

use bundlebase_connector::plugin::FfiConnector;
use bundlebase_connector::plugin::HttpConnector;
use bundlebase_connector::plugin::IpcConnector;
#[cfg(feature = "connector-kaggle")]
use bundlebase_connector::plugin::KaggleConnector;
#[cfg(feature = "connector-postgres")]
use bundlebase_connector::plugin::PostgresConnector;
use bundlebase_connector::plugin::RemoteDirConnector;
#[cfg(feature = "connector-web-scrape")]
use bundlebase_connector::plugin::WebScrapeConnector;

use crate::bundle::connector_entry::{self, ConnectorEntry};
use crate::BundlebaseError;
use std::collections::HashMap;
use std::sync::Arc;

// Re-export connector types from common
pub use bundlebase_common::connector::{
    ArgSpec, AttachedFileInfo, Connector, ConnectorSignature, DiscoveredLocation,
    FetchAction, FetchResults, FetchedBlock, MaterializedData, SourceData,
    format_fetch_summary,
};

/// Validate arguments against a connector signature.
///
/// Performs standard validation plus shared_utils copy arg validation.
pub fn validate_connector_args(
    func: &dyn Connector,
    args: &HashMap<String, String>,
) -> Result<(), BundlebaseError> {
    // Common's validation checks required/unknown args and copy-arg
    bundlebase_common::connector::validate_connector_args(args, &func.signature())?;
    // Then connector-specific validation
    func.validate_args(args)?;
    Ok(())
}

/// Registry for connectors.
///
/// Manages available connector implementations and connector entry definitions.
/// Built-in connectors are automatically registered on construction.
/// Connector entries (from IMPORT CONNECTOR) are managed via `add_entry`, `has_entry`, etc.
pub struct ConnectorRegistry {
    functions: HashMap<String, Arc<dyn Connector>>,
    entries: Vec<ConnectorEntry>,
}

impl ConnectorRegistry {
    /// Create a new registry with built-in connectors registered.
    ///
    /// Note: "ipc" and "ffi" are NOT registered here. They are only available
    /// via connectors (IMPORT CONNECTOR + SET CONNECTOR LOGIC). Use `create_instance()`
    /// to create instances of them when resolving connector entrypoints.
    pub fn new() -> Self {
        let mut registry = Self {
            functions: HashMap::new(),
            entries: Vec::new(),
        };

        // Register built-in connectors (ipc/native removed — only via defined sources)
        registry.register(Arc::new(HttpConnector));
        #[cfg(feature = "connector-kaggle")]
        registry.register(Arc::new(KaggleConnector));
        #[cfg(feature = "connector-postgres")]
        registry.register(Arc::new(PostgresConnector));
        registry.register(Arc::new(RemoteDirConnector));
        #[cfg(feature = "connector-web-scrape")]
        registry.register(Arc::new(WebScrapeConnector));

        registry
    }

    /// Register a connector.
    pub fn register(&mut self, func: Arc<dyn Connector>) {
        self.functions.insert(func.signature().name.clone(), func);
    }

    /// Get a connector by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Connector>> {
        self.functions.get(name).cloned()
    }

    /// Create a fresh instance for connectors that hold per-fetch state.
    ///
    /// For `Ipc`, returns a new `IpcConnector` with its own subprocess handle.
    /// For `Internal`, returns a new `FfiConnector`.
    pub fn create_instance(&self, runtime_type: crate::udf::RuntimeType) -> Option<Arc<dyn Connector>> {
        match runtime_type {
            crate::udf::RuntimeType::External => Some(Arc::new(IpcConnector::new())),
            crate::udf::RuntimeType::Internal => Some(Arc::new(FfiConnector::new())),
        }
    }

    /// Get all registered connector names.
    pub fn connector_names(&self) -> Vec<String> {
        self.functions.keys().cloned().collect()
    }

    // ==================== Connector entry management ====================

    /// Add a connector entry to the registry.
    pub fn add_entry(&mut self, entry: ConnectorEntry) {
        self.entries.push(entry);
    }

    /// Check if any connector entry exists for the given name.
    pub fn has_entry(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }

    /// Resolve the best connector entry for the current platform.
    pub fn resolve_entry(&self, name: &str) -> Result<ConnectorEntry, BundlebaseError> {
        connector_entry::resolve_connector(&self.entries, name)
    }

    /// Remove all connector entries for a name.
    pub fn remove_all_entries(&mut self, name: &str) {
        self.entries.retain(|e| e.name != name);
    }

    /// Remove matching connector entries. Returns the number removed.
    pub fn remove_entry(
        &mut self,
        name: &str,
        platform: Option<&crate::platform::Platform>,
        temporary_only: bool,
    ) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| {
            if e.name != name {
                return true;
            }
            if temporary_only && !e.temporary {
                return true;
            }
            if let Some(p) = platform {
                if &e.platform != p {
                    return true;
                }
            }
            false
        });
        before - self.entries.len()
    }

    /// Remove connector entries by their IDs.
    pub fn remove_entries_by_ids(&mut self, ids: &[crate::data::ObjectId]) {
        self.entries.retain(|e| !ids.contains(&e.id));
    }

    /// Rename connector entries matching the given IDs to a new name.
    pub fn rename_entries(&mut self, ids: &[crate::data::ObjectId], new_name: &crate::NamespacedName) {
        for entry in &mut self.entries {
            if ids.contains(&entry.id) {
                entry.name = new_name.clone();
            }
        }
    }

    /// Rename only temporary connector entries matching the old name to a new name.
    pub fn rename_temp_entries(&mut self, old_name: &str, new_name: &crate::NamespacedName) {
        for entry in &mut self.entries {
            if entry.temporary && entry.name == old_name {
                entry.name = new_name.clone();
            }
        }
    }

    /// Get a read-only view of all connector entries.
    pub fn entries(&self) -> &[ConnectorEntry] {
        &self.entries
    }

    /// Check if any temporary connector entries exist.
    pub fn has_temporary(&self) -> bool {
        self.entries.iter().any(|e| e.temporary)
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new() {
        let registry = ConnectorRegistry::new();
        assert!(registry.get("remote_dir").is_some());
        #[cfg(feature = "connector-web-scrape")]
        assert!(registry.get("web_scrape").is_some());
    }

    #[test]
    fn test_registry_get_remote_dir() {
        let registry = ConnectorRegistry::new();
        let func = registry.get("remote_dir").expect("remote_dir not found");
        assert_eq!(func.signature().name, "remote_dir");
    }

    #[cfg(feature = "connector-web-scrape")]
    #[test]
    fn test_registry_get_web_scrape() {
        let registry = ConnectorRegistry::new();
        let func = registry.get("web_scrape").expect("web_scrape not found");
        assert_eq!(func.signature().name, "web_scrape");
    }

    #[test]
    fn test_arg_spec() {
        let spec = ArgSpec {
            name: "url",
            description: "The URL",
            required: true,
            default: None,
        };
        assert_eq!(spec.name, "url");
        assert!(spec.required);
    }

    #[test]
    fn test_validate_args_unknown_arg() {
        let registry = ConnectorRegistry::new();
        let func = registry.get("remote_dir").expect("remote_dir not found");

        let mut args = HashMap::new();
        args.insert("url".to_string(), "file:///test/".to_string());
        args.insert("invalid_arg".to_string(), "value".to_string());

        let result = validate_connector_args(func.as_ref(), &args);
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("does not accept argument 'invalid_arg'"));
        assert!(err.contains("Valid arguments:"));
        assert!(err.contains("url (required)"));
    }

    #[test]
    fn test_validate_args_missing_required() {
        let registry = ConnectorRegistry::new();
        let func = registry.get("remote_dir").expect("remote_dir not found");

        let args = HashMap::new(); // Missing required 'url'

        let result = validate_connector_args(func.as_ref(), &args);
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("requires a 'url' argument"));
        assert!(err.contains("Valid arguments:"));
    }

    #[test]
    fn test_validate_args_valid() {
        let registry = ConnectorRegistry::new();
        let func = registry.get("remote_dir").expect("remote_dir not found");

        let mut args = HashMap::new();
        args.insert("url".to_string(), "file:///test/".to_string());
        args.insert("patterns".to_string(), "*.parquet".to_string());

        let result = validate_connector_args(func.as_ref(), &args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_discovered_location() {
        let loc = DiscoveredLocation {
            location: "subdir/file.parquet".to_string(),
            must_copy: false,
            format: "parquet".to_string(),
            version: "etag-123".to_string(),
        };
        assert_eq!(loc.location, "subdir/file.parquet");
        assert!(!loc.must_copy);
        assert_eq!(loc.format, "parquet");
        assert_eq!(loc.version, "etag-123");
    }

    #[test]
    fn test_connector_signature() {
        let sig = ConnectorSignature {
            name: "test".to_string(),
            arg_specs: vec![ArgSpec {
                name: "url",
                description: "The URL",
                required: true,
                default: None,
            }],
            accepts_extra_args: false,
        };
        assert_eq!(sig.name, "test");
        assert_eq!(sig.arg_specs.len(), 1);
    }

    #[test]
    fn test_ipc_ffi_removed_from_registry() {
        let registry = ConnectorRegistry::new();
        assert!(registry.get("ipc").is_none());
        assert!(registry.get("ffi").is_none());
    }

    #[test]
    fn test_ipc_native_still_in_create_instance() {
        use crate::udf::RuntimeType;
        let registry = ConnectorRegistry::new();
        assert!(registry.create_instance(RuntimeType::External).is_some());
        assert!(registry.create_instance(RuntimeType::Internal).is_some());
    }

    #[test]
    fn test_builtins_still_registered() {
        let registry = ConnectorRegistry::new();
        assert!(registry.get("remote_dir").is_some());
        #[cfg(feature = "connector-kaggle")]
        assert!(registry.get("kaggle").is_some());
        #[cfg(feature = "connector-web-scrape")]
        assert!(registry.get("web_scrape").is_some());
        #[cfg(feature = "connector-postgres")]
        assert!(registry.get("postgres").is_some());
    }
}
