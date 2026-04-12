//! Source struct representing a data source definition for a pack.

use crate::bundle::{BundleBuilder, CreateSourceOp};
use crate::connector::SaveAs;
use crate::data::ObjectId;
use crate::source::{AttachedFileInfo, FetchAction, ConnectorRegistry, SyncMode, orchestrate_fetch};
use crate::BundlebaseError;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

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
    args: HashMap<String, String>,
    /// How to save fetched data.
    save_as: SaveAs,
    /// Optional size threshold in bytes for batching small files together.
    batch_bytes: Option<usize>,
    /// Attached files from this source, keyed by source_location
    attached_files: RwLock<HashMap<String, AttachedFileInfo>>,
}

impl Source {
    pub fn new(
        id: ObjectId,
        pack: ObjectId,
        connector: String,
        args: HashMap<String, String>,
        save_as: SaveAs,
    ) -> Self {
        Self::new_with_options(id, pack, connector, args, save_as, None)
    }

    pub fn new_with_options(
        id: ObjectId,
        pack: ObjectId,
        connector: String,
        args: HashMap<String, String>,
        save_as: SaveAs,
        batch_bytes: Option<usize>,
    ) -> Self {
        Self {
            id,
            pack,
            connector: RwLock::new(connector),
            args,
            save_as,
            batch_bytes,
            attached_files: RwLock::new(HashMap::new()),
        }
    }

    pub fn from_op(
        op: &CreateSourceOp,
        registry: &ConnectorRegistry,
    ) -> Result<Self, BundlebaseError> {
        if !op.connector.contains('.') {
            registry
                .get(&op.connector)
                .ok_or_else(|| format!("Unknown connector '{}'", op.connector))?;
        }

        let save_as = op.save_as.as_deref()
            .map(SaveAs::parse)
            .transpose()?
            .unwrap_or(SaveAs::Auto);

        Ok(Self::new_with_options(
            op.id,
            op.pack,
            op.connector.clone(),
            op.args.clone(),
            save_as,
            op.batch_bytes,
        ))
    }

    pub fn batch_bytes(&self) -> Option<usize> {
        self.batch_bytes
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

    pub fn save_as(&self) -> &SaveAs {
        &self.save_as
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
            let entry = bundle
                .connector_registry()
                .read()
                .resolve_entry(&connector_name)
                .map_err(|_| {
                    BundlebaseError::from(format!(
                        "Connector '{}' is not available. If it was imported as a temporary \
                         connector in a previous session, re-import it with:\n  \
                         IMPORT TEMP CONNECTOR {} FROM '<runtime>::<entrypoint>'\n\
                         Or import it permanently with:\n  \
                         IMPORT CONNECTOR {} FROM '<runtime>::<entrypoint>'",
                        connector_name, connector_name, connector_name
                    ))
                })?;

            let runtime_type = entry.from.runtime_type();
            let registry = bundle.connector_registry();
            let reg = registry.read();
            let func = reg
                .create_instance(runtime_type)
                .ok_or_else(|| format!("Unknown connector type '{:?}'", runtime_type))?;

            // Resolve bundle-relative entrypoint paths against the data directory
            let resolved_from = entry.from.resolve_path(&bundle.data_dir());

            // Merge: inject "call" from entrypoint (reconstructed with prefix), then overlay user args
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

        orchestrate_fetch(
            func.as_ref(),
            &resolved_args,
            mode,
            &self.save_as,
            data_dir.as_ref(),
            &attached_files,
            &(Arc::clone(&config) as Arc<dyn crate::ConfigProvider>),
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
            SaveAs::Auto,
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
            save_as: None,
            batch_bytes: None,
            expected_schema: None,
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
            save_as: None,
            batch_bytes: None,
            expected_schema: None,
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
            save_as: None,
            batch_bytes: None,
            expected_schema: None,
        };

        let result = Source::from_op(&op, &registry);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown connector"));
    }
}
