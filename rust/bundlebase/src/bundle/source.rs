//! Source struct representing a data source definition for a pack.

use crate::bundle::{BundleBuilder, CreateSourceOp};
use crate::data::ObjectId;
use crate::source::{AttachedFileInfo, FetchAction, ConnectorRegistry, SyncMode, orchestrate_fetch};
use crate::source::connector_utils;
use crate::BundlebaseError;
use parking_lot::RwLock;
use std::collections::HashMap;

/// Represents a data source definition for a pack.
///
/// A source specifies how to discover and list data files.
/// All configuration is stored in connector-specific arguments.
#[derive(Debug)]
pub struct Source {
    id: ObjectId,
    pack: ObjectId,
    /// Connector name (e.g., "remote_dir" for built-in, "acme.weather" for custom)
    connector: RwLock<String>,
    /// Connector-specific configuration arguments
    /// For "remote_dir": "url" (required), "patterns" (optional)
    args: HashMap<String, String>,
    /// Attached files from this source, keyed by source_location
    attached_files: RwLock<HashMap<String, AttachedFileInfo>>,
}

impl Source {
    pub fn new(
        id: ObjectId,
        pack: ObjectId,
        connector: String,
        args: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            pack,
            connector: RwLock::new(connector),
            args,
            attached_files: RwLock::new(HashMap::new()),
        }
    }

    pub fn from_op(
        op: &CreateSourceOp,
        registry: &ConnectorRegistry,
    ) -> Result<Self, BundlebaseError> {
        if !op.connector.contains('.') {
            // Built-in connector: validate it exists in the registry
            registry
                .get(&op.connector)
                .ok_or_else(|| format!("Unknown connector '{}'", op.connector))?;
        }
        // Dotted names are validated at check() time against source_definitions

        Ok(Self::new(
            op.id,
            op.pack,
            op.connector.clone(),
            op.args.clone(),
        ))
    }

    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    pub fn pack(&self) -> &ObjectId {
        &self.pack
    }

    pub fn connector(&self) -> String {
        self.connector.read().clone()
    }

    /// Update the connector name for this source.
    ///
    /// Used when a connector is renamed to cascade the name change to sources.
    pub fn set_connector_name(&self, name: String) {
        *self.connector.write() = name;
    }

    pub fn args(&self) -> &HashMap<String, String> {
        &self.args
    }

    /// Fetch this source: find new data and materialize it.
    ///
    /// Returns a list of fetch actions (Add, Replace, Remove) based on the sync mode.
    pub async fn fetch(
        &self,
        builder: &BundleBuilder,
        mode: SyncMode,
    ) -> Result<Vec<FetchAction>, BundlebaseError> {
        let connector_name = self.connector();
        let (func, data_dir, config, resolved_args) = if connector_name.contains('.') {
            // Defined source: resolve connector entry for current platform
            let bundle = builder.bundle();
            let entry = bundle.connector_registry().read().resolve_entry(&connector_name)?;

            let runtime_type = entry.from.runtime_type();
            let registry = bundle.connector_registry();
            let reg = registry.read();
            let func = reg
                .create_instance(runtime_type)
                .ok_or_else(|| format!("Unknown connector type '{:?}'", runtime_type))?;

            // Resolve bundle-relative logic paths against the data directory
            let resolved_from = entry.from.resolve_path(&bundle.data_dir());

            // Merge: inject "call" from logic (reconstructed with prefix), then overlay user args
            let mut merged_args = self.args.clone();
            let call_string = resolved_from.build_call_string();
            merged_args.insert("call".to_string(), call_string);

            (func, bundle.data_dir(), bundle.config(), merged_args)
        } else {
            // Built-in function: use directly
            let bundle = builder.bundle();
            let registry = bundle.connector_registry();
            let reg = registry.read();
            let func = reg
                .get(&connector_name)
                .ok_or_else(|| format!("Unknown connector '{}'", connector_name))?;
            (func, bundle.data_dir(), bundle.config(), self.args.clone())
        };

        // Get attached files directly from self
        let attached_files = self.attached_files();
        let should_copy = connector_utils::should_copy(&resolved_args);

        orchestrate_fetch(
            func.as_ref(),
            &resolved_args,
            mode,
            should_copy,
            data_dir.as_ref(),
            &attached_files,
            &config,
        )
        .await
    }

    /// Get attached files with metadata for change detection.
    /// Returns a clone of the internal HashMap.
    pub fn attached_files(&self) -> HashMap<String, AttachedFileInfo> {
        self.attached_files.read().clone()
    }

    /// Add an attached file to this source.
    pub(crate) fn add_attached_file(&self, source_location: &str, info: AttachedFileInfo) {
        self.attached_files
            .write()
            .insert(source_location.to_string(), info);
    }

    /// Remove an attached file from this source.
    pub(crate) fn remove_attached_file(&self, source_location: &str) {
        self.attached_files.write().remove(source_location);
    }

    /// Update an attached file in this source.
    pub(crate) fn update_attached_file(&self, source_location: &str, info: AttachedFileInfo) {
        self.attached_files
            .write()
            .insert(source_location.to_string(), info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_args(url: &str, patterns: Option<&str>) -> HashMap<String, String> {
        let mut args = HashMap::new();
        args.insert("url".to_string(), url.to_string());
        if let Some(p) = patterns {
            args.insert("patterns".to_string(), p.to_string());
        }
        args
    }

    #[test]
    fn test_new_source() {
        let id = ObjectId::generate();
        let pack = ObjectId::generate();
        let source = Source::new(
            id,
            pack,
            "remote_dir".to_string(),
            make_args("s3://bucket/data/", Some("**/*")),
        );

        assert_eq!(source.id(), &id);
        assert_eq!(source.pack(), &pack);
        assert_eq!(source.args().get("url").map(|s| s.as_str()), Some("s3://bucket/data/"));
        assert_eq!(source.args().get("patterns").map(|s| s.as_str()), Some("**/*"));
        assert_eq!(source.connector(), "remote_dir");
    }

    #[test]
    fn test_from_op() {
        let registry = ConnectorRegistry::new();

        let id = ObjectId::generate();
        let pack = ObjectId::generate();
        let op = CreateSourceOp {
            id,
            pack,
            connector: "remote_dir".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.parquet")),
        };

        let source = Source::from_op(&op, &registry).unwrap();
        assert_eq!(source.id(), &id);
        assert_eq!(source.pack(), &pack);
        assert_eq!(source.args().get("url").map(|s| s.as_str()), Some("s3://bucket/data/"));
        assert_eq!(source.args().get("patterns").map(|s| s.as_str()), Some("**/*.parquet"));
        assert_eq!(source.connector(), "remote_dir");
    }

    #[test]
    fn test_from_op_with_extra_args() {
        let registry = ConnectorRegistry::new();
        let mut args = make_args("s3://bucket/data/", None);
        args.insert("key".to_string(), "value".to_string());

        let op = CreateSourceOp {
            id: ObjectId::generate(),
            pack: ObjectId::generate(),
            connector: "remote_dir".to_string(),
            args: args.clone(),
        };

        // from_op succeeds, validation happens in check()
        let result = Source::from_op(&op, &registry);
        assert!(result.is_ok());
        let source = result.unwrap();
        assert_eq!(source.args(), &args);
    }

    #[test]
    fn test_from_op_unknown_function() {
        let registry = ConnectorRegistry::new();

        let op = CreateSourceOp {
            id: ObjectId::generate(),
            pack: ObjectId::generate(),
            connector: "unknown_function".to_string(),
            args: HashMap::new(),
        };

        let result = Source::from_op(&op, &registry);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown connector"));
    }
}
