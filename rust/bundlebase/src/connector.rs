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

pub mod plugin;

use plugin::FfiConnector;
use plugin::HttpConnector;
use plugin::IpcConnector;
use plugin::KaggleConnector;
use plugin::PostgresConnector;
use plugin::RemoteDirConnector;
use plugin::WebScrapeConnector;
use crate::source::shared_utils;

use crate::bundle::connector_entry::{self, ConnectorEntry};
use crate::{BundleConfig, BundlebaseError};
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{self, BoxStream};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Data returned by a connector's `data()` method.
///
/// Sources that produce structured data (e.g., database queries, IPC subprocess)
/// return `Arrow` batch streams, and the orchestration layer handles Parquet serialization.
/// Sources that provide raw file bytes (e.g., Kaggle downloads, SFTP files)
/// return `RawBytes` as a stream, which is written directly as-is.
pub enum SourceData {
    /// Stream of Arrow RecordBatches (will be converted to Parquet by the orchestrator).
    Arrow(BoxStream<'static, Result<RecordBatch, BundlebaseError>>),
    /// Raw byte stream (written directly as-is).
    RawBytes(BoxStream<'static, Result<Bytes, std::io::Error>>),
}

impl SourceData {
    /// Create an Arrow variant from a single RecordBatch.
    ///
    /// Convenience constructor that wraps a batch in a single-element stream.
    pub fn from_batch(batch: RecordBatch) -> Self {
        SourceData::Arrow(Box::pin(stream::once(async { Ok(batch) })))
    }

    /// Create a RawBytes variant from in-memory bytes.
    ///
    /// Convenience constructor that wraps `Bytes` in a single-element stream.
    pub fn from_bytes(bytes: Bytes) -> Self {
        SourceData::RawBytes(Box::pin(stream::once(async { Ok(bytes) })))
    }
}

/// Describes a connector argument for documentation and validation.
#[derive(Debug, Clone)]
pub struct ArgSpec {
    /// Argument name (key in the args HashMap)
    pub name: &'static str,
    /// Human-readable description
    pub description: &'static str,
    /// Whether this argument is required
    pub required: bool,
    /// Default value if not provided (None means no default)
    pub default: Option<&'static str>,
}

/// Signature of a connector: its name and argument specifications.
#[derive(Debug, Clone)]
pub struct ConnectorSignature {
    /// Unique name for this connector (e.g., "remote_dir")
    pub name: String,
    /// Argument declarations
    pub arg_specs: Vec<ArgSpec>,
    /// When true, unknown arguments are allowed (forwarded to the bridge).
    pub accepts_extra_args: bool,
}

/// A partition of source data discovered during the discovery phase.
///
/// Each `DiscoveredLocation` becomes one block in the bundle. For file-based
/// sources (remote_dir, web_scrape, kaggle), partitions map naturally to
/// individual files. For structured sources (postgres, APIs), the connector
/// decides how to divide data into partitions — for example,
/// postgres partitions query results into row-range chunks based on a
/// sort column.
///
/// **Partition stability matters.** During sync, the orchestrator matches
/// discovered locations against previously-attached blocks by `location`
/// string. Stable partitioning means unchanged data keeps its existing
/// blocks (no re-fetch), new data appears as new blocks, and modified or
/// removed partitions are detected via `version` changes or absence.
/// If partitioning is unstable (e.g., row boundaries shift when data is
/// inserted), blocks appear changed even when the underlying data hasn't.
#[derive(Debug, Clone)]
pub struct DiscoveredLocation {
    /// Stable identifier that describes what data this partition contains.
    ///
    /// The location should be meaningful to the data's natural structure:
    /// a file-based source uses a path or URL, a database uses a key range,
    /// a spreadsheet might use a cell range. The identifier must be consistent
    /// across `discover()` calls so the orchestrator can match partitions to
    /// previously-attached blocks during sync.
    ///
    /// Examples:
    /// - **remote_dir:** relative file path (e.g., `"subdir/data.parquet"`)
    /// - **kaggle:** filename (e.g., `"train.csv"`)
    /// - **postgres:** JSON range (e.g., `{"sort_col":"id","min":"1","max":"1000"}`)
    /// - **spreadsheet:** cell range (e.g., `"Sheet1!A1:Z500"`)
    /// - **ipc:** subprocess-defined identifier
    pub location: String,
    /// Whether this location requires copying data into the bundle's data directory.
    /// True for sources without stable URLs (e.g., Kaggle, Postgres).
    pub must_copy: bool,
    /// Data format / file extension (e.g., "parquet", "csv").
    pub format: String,
    /// Source-specific version string used for change detection during sync.
    ///
    /// The orchestrator compares this against the version stored when the block
    /// was last attached. A mismatch triggers a re-fetch of this partition.
    ///
    /// The meaning varies by connector:
    /// - **remote_dir:** file metadata such as ETag or Last-Modified
    /// - **web_scrape:** HTTP ETag or Last-Modified
    /// - **kaggle:** dataset version number (e.g., `"42"`)
    /// - **postgres:** `"count:checksum"` — row count and content hash
    /// - **ipc:** subprocess-defined version string
    pub version: String,
}

/// Result of materializing a single data unit from a source.
#[derive(Debug, Clone)]
pub struct MaterializedData {
    /// Location of the materialized file (URL in data_dir or original if not copied)
    pub attach_location: String,
    /// Original source location identifier (relative path or row range for storage)
    pub source_location: String,
    /// Full URL to the source file (may differ from source_location)
    pub source_url: String,
    /// SHA256 hash of the content (full 64-character hex string).
    /// None if the hash is not yet known (will be computed during attach).
    pub hash: Option<String>,
    /// Source-specific version string used for change detection in Update/Sync modes.
    pub version: String,
}

/// Metadata about an attached file from a source.
///
/// Used during fetch to compare remote files with already-attached files.
#[derive(Debug, Clone)]
pub struct AttachedFileInfo {
    /// The location where this block is currently stored
    pub location: String,
    /// Version string from AttachBlockOp (ETag/S3 version/mtime hash)
    pub version: String,
    /// File size in bytes (from AttachBlockOp.bytes)
    pub bytes: Option<usize>,
}

/// Action to take for a discovered file during fetch.
#[derive(Debug, Clone)]
pub enum FetchAction {
    /// Attach a new file
    Add(MaterializedData),
    /// Replace an existing file that has changed
    Replace {
        /// The source_location of the old block to detach
        old_source_location: String,
        /// The new materialized data to attach
        data: MaterializedData,
    },
    /// Detach a file that no longer exists remotely
    Remove {
        /// The source_location of the block to detach
        source_location: String,
    },
}

/// Information about a block that was fetched (added or replaced).
#[derive(Debug, Clone)]
pub struct FetchedBlock {
    /// Location where the block is attached (path in data_dir or URL)
    pub attach_location: String,
    /// Original source location identifier
    pub source_location: String,
}

/// Results from fetching a single source.
///
/// Contains information about the source and all blocks that were
/// added, replaced, or removed during the fetch operation.
#[derive(Debug, Clone)]
pub struct FetchResults {
    /// Connector name (e.g., "remote_dir", "web_scrape")
    pub connector: String,
    /// Source URL or identifier from args
    pub source_url: String,
    /// Pack name ("base" or join name)
    pub pack: String,
    /// Blocks that were newly added
    pub added: Vec<FetchedBlock>,
    /// Blocks that were replaced (updated)
    pub replaced: Vec<FetchedBlock>,
    /// Source locations of blocks that were removed
    pub removed: Vec<String>,
}

impl FetchResults {
    /// Create a new FetchResults for a source with no changes.
    pub fn empty(connector: String, source_url: String, pack: String) -> Self {
        Self {
            connector,
            source_url,
            pack,
            added: Vec::new(),
            replaced: Vec::new(),
            removed: Vec::new(),
        }
    }

    /// Create FetchResults from a list of FetchActions.
    pub fn from_actions(
        connector: String,
        source_url: String,
        pack: String,
        actions: Vec<FetchAction>,
    ) -> Self {
        let mut added = Vec::new();
        let mut replaced = Vec::new();
        let mut removed = Vec::new();

        for action in actions {
            match action {
                FetchAction::Add(data) => {
                    added.push(FetchedBlock {
                        attach_location: data.attach_location,
                        source_location: data.source_location,
                    });
                }
                FetchAction::Replace { data, .. } => {
                    replaced.push(FetchedBlock {
                        attach_location: data.attach_location,
                        source_location: data.source_location,
                    });
                }
                FetchAction::Remove { source_location } => {
                    removed.push(source_location);
                }
            }
        }

        Self {
            connector,
            source_url,
            pack,
            added,
            replaced,
            removed,
        }
    }

    /// Total number of actions (added + replaced + removed).
    pub fn total_count(&self) -> usize {
        self.added.len() + self.replaced.len() + self.removed.len()
    }

    /// Check if there were any changes.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.replaced.is_empty() && self.removed.is_empty()
    }

    /// Format a single result for display.
    pub fn summary(&self) -> String {
        let changes = self.added.len() + self.replaced.len() + self.removed.len();
        format!("{}: {} changes", self.pack, changes)
    }
}

/// Format a slice of FetchResults for display.
pub fn format_fetch_summary(results: &[FetchResults]) -> String {
    if results.is_empty() {
        "No sources to fetch from".to_string()
    } else {
        let summary: Vec<String> = results.iter().map(|r| r.summary()).collect();
        format!("Fetched: {}", summary.join(", "))
    }
}

/// Trait for connector implementations.
///
/// Connectors define how data is discovered and made available.
/// Each connector controls:
/// - What "location" means (file path, row range, filename, etc.)
/// - How to discover all locations with version info
/// - How to provide data (either raw bytes or a stable URL)
///
/// ## Implementing a Connector
///
/// Provide all four methods:
/// - `signature()` - Name and argument specs
/// - `discover()` - Find all locations
/// - `data()` - Return raw bytes (or None to use stable_url)
/// - `stable_url()` - Return a downloadable URL (or None to use data)
///
/// At least one of `data()` or `stable_url()` must return Some for each location.
#[async_trait]
pub trait Connector: Send + Sync {
    /// Return the signature (name + arg specs) for this connector.
    fn signature(&self) -> ConnectorSignature;

    /// Custom validation for function-specific arguments.
    ///
    /// Called after standard validation (required/unknown/copy checks).
    /// Override to add custom validation. Default does nothing.
    fn validate_args(&self, _args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        Ok(())
    }

    /// Discover all data partitions from the source.
    ///
    /// Each returned [`DiscoveredLocation`] defines one partition of source data
    /// that will become a block in the bundle. For file-based sources this means
    /// listing files; for structured sources (databases, APIs) this means deciding
    /// how to divide data into manageable, stable chunks whose `location` strings
    /// describe the data they contain (e.g., a key range for a database table,
    /// a cell range for a spreadsheet, a date range for time-series data).
    ///
    /// **Partitioning should be as stable as possible.** The orchestrator matches
    /// locations by their `location` string against previously-attached blocks.
    /// Stable partitions let sync detect exactly which blocks have changed, which
    /// are new, and which have been removed — without re-fetching unchanged data.
    /// For example, a database source that partitions by ID ranges will correctly
    /// detect that existing ranges are unchanged while new rows appear as new
    /// partitions, whereas partitioning by row offset would cause every block to
    /// appear changed whenever rows are inserted.
    ///
    /// Returns ALL matching locations (including already-attached ones).
    /// The orchestration layer handles filtering based on sync mode.
    ///
    /// # Arguments
    /// * `args` - Source configuration arguments
    /// * `attached_locations` - Locations already attached (for optional optimization)
    /// * `config` - Bundle configuration (credentials, etc.)
    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError>;

    /// Provide data for a discovered location.
    ///
    /// Return `Ok(Some(SourceData::Arrow(batches)))` for structured data (converted to Parquet by orchestrator).
    /// Return `Ok(Some(SourceData::RawBytes(bytes)))` for raw file bytes (written as-is).
    /// Return `Ok(None)` if data should be fetched via `stable_url()` instead.
    ///
    /// ## Reserved args keys
    ///
    /// The `args` map may contain reserved keys prefixed with `_`. Connectors
    /// MAY check for these to enable optional optimizations:
    ///
    /// - **`_columns`** — A comma-separated list of column names that the caller
    ///   is interested in (e.g., `"col1,col2,col3"`). Connectors that support
    ///   column pushdown can use this to fetch only the requested columns,
    ///   reducing data transfer and processing. Connectors that do not support
    ///   column pushdown can safely ignore this key.
    async fn data(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Option<SourceData>, BundlebaseError>;

    /// Provide a stable URL where the data can be downloaded.
    ///
    /// Return `Ok(Some(Url))` with the stable download URL.
    /// Return `Ok(None)` if data is only available via `data()`.
    async fn stable_url(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Option<Url>, BundlebaseError>;
}

/// Validate arguments against a connector signature.
///
/// Performs standard validation:
/// - Checks that all required arguments are present
/// - Checks for unknown arguments
/// - Validates the `copy` argument if present
/// - Calls the connector's custom `validate_args` method
pub fn validate_connector_args(
    func: &dyn Connector,
    args: &HashMap<String, String>,
) -> Result<(), BundlebaseError> {
    let sig = func.signature();
    validate_args_standard(&sig, args)?;
    func.validate_args(args)?;
    Ok(())
}

/// Standard argument validation against a signature.
///
/// Checks required arguments, validates unknown arguments, and validates `copy` argument.
fn validate_args_standard(
    signature: &ConnectorSignature,
    args: &HashMap<String, String>,
) -> Result<(), BundlebaseError> {
    let specs = &signature.arg_specs;
    let valid_names: HashSet<&str> = specs.iter().map(|s| s.name).collect();

    // Check for required arguments
    for spec in specs {
        if spec.required && !args.contains_key(spec.name) {
            let valid_args = format_arg_list(specs);
            return Err(format!(
                "Function '{}' requires a '{}' argument. Valid arguments: {}",
                signature.name, spec.name, valid_args
            )
            .into());
        }
    }

    // Check for unknown arguments (skip if the source accepts extra args).
    // Keys prefixed with "_" are reserved system keys and always allowed.
    if !signature.accepts_extra_args {
        for key in args.keys() {
            if !key.starts_with('_') && !valid_names.contains(key.as_str()) {
                let valid_args = format_arg_list(specs);
                return Err(format!(
                    "Function '{}' does not accept argument '{}'. Valid arguments: {}",
                    signature.name, key, valid_args
                )
                .into());
            }
        }
    }

    shared_utils::validate_copy_arg(&signature.name, args)
}

/// Format arg specs as a human-readable list for error messages.
fn format_arg_list(specs: &[ArgSpec]) -> String {
    let items: Vec<String> = specs
        .iter()
        .map(|s| {
            if s.required {
                format!("{} (required)", s.name)
            } else if let Some(default) = s.default {
                format!("{} (optional, default: {})", s.name, default)
            } else {
                format!("{} (optional)", s.name)
            }
        })
        .collect();
    items.join(", ")
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
        registry.register(Arc::new(KaggleConnector));
        registry.register(Arc::new(PostgresConnector));
        registry.register(Arc::new(RemoteDirConnector));
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
        assert!(registry.get("web_scrape").is_some());
    }

    #[test]
    fn test_registry_get_remote_dir() {
        let registry = ConnectorRegistry::new();
        let func = registry.get("remote_dir").expect("remote_dir not found");
        assert_eq!(func.signature().name, "remote_dir");
    }

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
        assert!(registry.get("kaggle").is_some());
        assert!(registry.get("web_scrape").is_some());
        assert!(registry.get("postgres").is_some());
    }
}
