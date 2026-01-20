//! Source function system for data discovery and materialization.
//!
//! Source functions define how data is discovered and materialized into files.
//! Different implementations can provide different strategies (e.g., directory listing,
//! database queries, API pagination, web scraping, etc.).
//!
//! ## Architecture
//!
//! The `SourceFunction` trait separates concerns:
//! - `discover()` - Find new data locations (URLs, row ranges, etc.)
//! - `materialize()` - Download/copy data to the bundle's data directory
//! - `fetch()` - Orchestrates discovery and materialization (default impl provided)
//!
//! Most implementations only need to implement `discover()`. The default `materialize()`
//! and `fetch()` implementations handle the common case.

use super::postgres::PostgresFunction;
use super::remote_dir::RemoteDirFunction;
use super::source_utils;
use super::web_scrape::WebScrapeFunction;

// Re-export MaterializeResult for use by source function implementations
pub use super::source_utils::MaterializeResult;
use crate::io::IOReadWriteDir;
use crate::{BundleConfig, BundlebaseError};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Describes a source function argument for documentation and validation.
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

/// A discovered location ready for materialization.
///
/// Represents a unit of data found by the source function's discovery phase.
#[derive(Debug, Clone)]
pub struct DiscoveredLocation {
    /// URL to fetch the data from
    pub url: Url,
    /// Identifier to track this location (stored in AttachBlockOp.source_location)
    /// Often the same as url.to_string(), but can differ (e.g., for normalized URLs)
    pub source_location: String,
}

impl DiscoveredLocation {
    /// Create a new discovered location where source_location equals the URL string.
    pub fn from_url(url: Url) -> Self {
        let source_location = url.to_string();
        Self { url, source_location }
    }
}

/// Result of materializing a single data unit from a source.
#[derive(Debug, Clone)]
pub struct MaterializedData {
    /// Location of the materialized file (URL in data_dir or original if not copied)
    pub attach_location: String,
    /// Original source location identifier (relative path or row range for storage)
    pub source_location: String,
    /// Full URL to the source file for version reading (may differ from source_location)
    pub source_url: String,
    /// SHA256 hash of the content (full 64-character hex string)
    pub hash: String,
}

/// Sync mode for source fetch operations.
///
/// Controls how fetch handles existing files when checking for updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// Only add new files (default behavior)
    #[default]
    Add,
    /// Add new files and replace changed files
    Update,
    /// Add new files, replace changed files, and remove missing files
    Sync,
}

impl SyncMode {
    /// Parse sync mode from string argument.
    pub fn from_arg(value: &str) -> Result<Self, BundlebaseError> {
        match value.to_lowercase().as_str() {
            "add" => Ok(SyncMode::Add),
            "update" => Ok(SyncMode::Update),
            "sync" => Ok(SyncMode::Sync),
            _ => Err(format!(
                "Invalid mode '{}'. Must be 'add', 'update', or 'sync'",
                value
            )
            .into()),
        }
    }
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
    /// Source function name (e.g., "remote_dir", "web_scrape")
    pub source_function: String,
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
    pub fn empty(source_function: String, source_url: String, pack: String) -> Self {
        Self {
            source_function,
            source_url,
            pack,
            added: Vec::new(),
            replaced: Vec::new(),
            removed: Vec::new(),
        }
    }

    /// Create FetchResults from a list of FetchActions.
    pub fn from_actions(
        source_function: String,
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
            source_function,
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

/// Trait for source function implementations.
///
/// Source functions define how data is discovered and materialized.
/// Each source function controls:
/// - What "location" means (file URL, row range, API cursor, etc.)
/// - How to discover new locations
/// - How to materialize data into files (optional override)
///
/// ## Implementing a Source Function
///
/// Most implementations only need to provide:
/// - `name()` - Unique identifier
/// - `arg_specs()` - Argument declarations
/// - `discover()` - Find new data locations
///
/// The default implementations handle validation, materialization, and the fetch loop.
///
/// ## Example
///
/// ```ignore
/// impl SourceFunction for MySourceFunction {
///     fn name(&self) -> &str { "my_source" }
///
///     fn arg_specs(&self) -> Vec<ArgSpec> {
///         vec![
///             ArgSpec { name: "url", description: "Source URL", required: true, default: None },
///         ]
///     }
///
///     async fn discover(&self, args: &HashMap<String, String>, attached: &HashSet<String>, config: &BundleConfig)
///         -> Result<Vec<DiscoveredLocation>, BundlebaseError>
///     {
///         // Find and return new locations...
///     }
/// }
/// ```
#[async_trait]
pub trait SourceFunction: Send + Sync {
    /// Name of this source function (e.g., "remote_dir", "web_scrape")
    fn name(&self) -> &str;

    /// Declare arguments this function accepts.
    ///
    /// Used for documentation, validation, and potential UI generation.
    /// Default: empty (no declared arguments).
    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![]
    }

    /// Validate arguments for this function.
    ///
    /// Default implementation checks required arguments from `arg_specs()`
    /// and validates the `copy` argument if present.
    ///
    /// Override to add custom validation (call default first via `default_validate_args`).
    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        self.default_validate_args(args)
    }

    /// Default argument validation logic.
    ///
    /// Checks required arguments, validates unknown arguments, and validates `copy` argument.
    /// Call this from custom `validate_args` implementations.
    fn default_validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        let specs = self.arg_specs();
        let valid_names: HashSet<&str> = specs.iter().map(|s| s.name).collect();

        // Check for required arguments
        for spec in &specs {
            if spec.required && !args.contains_key(spec.name) {
                let valid_args: Vec<String> = specs
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
                return Err(format!(
                    "Function '{}' requires a '{}' argument. Valid arguments: {}",
                    self.name(),
                    spec.name,
                    valid_args.join(", ")
                )
                .into());
            }
        }

        // Check for unknown arguments
        for key in args.keys() {
            if !valid_names.contains(key.as_str()) {
                let valid_args: Vec<String> = specs
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
                return Err(format!(
                    "Function '{}' does not accept argument '{}'. Valid arguments: {}",
                    self.name(),
                    key,
                    valid_args.join(", ")
                )
                .into());
            }
        }

        source_utils::validate_copy_arg(self.name(), args)
    }

    /// Discover new data locations.
    ///
    /// This is the core method that each source function must implement.
    /// It should:
    /// 1. Query/list/scrape the source to find all available locations
    /// 2. Filter out locations already in `attached_locations`
    /// 3. Return the new locations to be materialized
    ///
    /// # Arguments
    /// * `args` - Source configuration arguments
    /// * `attached_locations` - Locations already attached from this source
    /// * `config` - Bundle configuration (credentials, etc.)
    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError>;

    /// Materialize a single discovered location.
    ///
    /// Downloads/copies the data to `data_dir` and returns a MaterializeResult
    /// containing the file reference and its SHA256 hash.
    ///
    /// Default implementation uses `source_utils::materialize_url` which handles
    /// HTTP(S) via reqwest and other schemes via IOFile.
    ///
    /// Override for special protocols (SFTP, FTP) or custom handling.
    async fn materialize(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        data_dir: &dyn IOReadWriteDir,
        config: &Arc<BundleConfig>,
    ) -> Result<MaterializeResult, BundlebaseError> {
        let should_copy = source_utils::should_copy(args);
        source_utils::materialize_url(&location.url, should_copy, data_dir, config).await
    }

    /// Fetch the source: discover new data and materialize it.
    ///
    /// Default implementation orchestrates the discover/materialize loop.
    /// Most implementations should not need to override this.
    ///
    /// # Arguments
    /// * `args` - Source configuration
    /// * `attached_locations` - Locations already attached from this source
    /// * `data_dir` - Where to write materialized files
    /// * `config` - Bundle configuration
    ///
    /// # Returns
    /// List of materialized data ready for attachment
    async fn fetch(
        &self,
        args: &HashMap<String, String>,
        attached_locations: HashSet<String>,
        data_dir: &dyn IOReadWriteDir,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<MaterializedData>, BundlebaseError> {
        let discovered = self.discover(args, &attached_locations, &config).await?;

        let mut results = Vec::with_capacity(discovered.len());
        for location in discovered {
            let source_url = location.url.to_string();
            let result = self.materialize(&location, args, data_dir, &config).await?;
            // Use relative path if file is in data_dir, otherwise full URL
            let attach_location = data_dir
                .relative_path(result.file.as_ref())
                .unwrap_or_else(|_| result.file.url().to_string());
            results.push(MaterializedData {
                attach_location,
                source_location: location.source_location,
                source_url,
                hash: result.hash,
            });
        }

        Ok(results)
    }

    /// Fetch the source with sync mode support.
    ///
    /// This method extends fetch() to support update and sync modes:
    /// - `Add`: Only add new files (same as fetch())
    /// - `Update`: Add new files and replace files that have changed
    /// - `Sync`: Add new, replace changed, and remove files no longer at source
    ///
    /// Default implementation delegates to fetch() for Add mode and returns
    /// an error for other modes. Source functions that support update/sync modes
    /// should override this method.
    ///
    /// # Arguments
    /// * `args` - Source configuration
    /// * `attached_files` - Map of source_location to AttachedFileInfo for already-attached files
    /// * `data_dir` - Where to write materialized files
    /// * `config` - Bundle configuration
    /// * `mode` - Sync mode to use
    ///
    /// # Returns
    /// List of fetch actions (Add, Replace, Remove)
    async fn fetch_with_mode(
        &self,
        args: &HashMap<String, String>,
        attached_files: &HashMap<String, AttachedFileInfo>,
        data_dir: &dyn IOReadWriteDir,
        config: Arc<BundleConfig>,
        mode: SyncMode,
    ) -> Result<Vec<FetchAction>, BundlebaseError> {
        match mode {
            SyncMode::Add => {
                // For Add mode, delegate to existing fetch() behavior
                let attached_locations: HashSet<String> = attached_files.keys().cloned().collect();
                let materialized = self.fetch(args, attached_locations, data_dir, config).await?;
                Ok(materialized.into_iter().map(FetchAction::Add).collect())
            }
            SyncMode::Update | SyncMode::Sync => {
                // Default implementation doesn't support update/sync
                Err(format!(
                    "Source function '{}' does not support mode '{:?}'. Only 'add' mode is supported.",
                    self.name(),
                    mode
                )
                .into())
            }
        }
    }
}

/// Registry for source functions.
///
/// Manages available source functions and provides lookup by name.
/// Built-in functions are automatically registered on construction.
pub struct SourceFunctionRegistry {
    functions: HashMap<String, Arc<dyn SourceFunction>>,
}

impl SourceFunctionRegistry {
    /// Create a new registry with built-in functions registered.
    pub fn new() -> Self {
        let mut registry = Self {
            functions: HashMap::new(),
        };

        // Register built-in functions
        registry.register(Arc::new(PostgresFunction));
        registry.register(Arc::new(RemoteDirFunction));
        registry.register(Arc::new(WebScrapeFunction));

        registry
    }

    /// Register a source function.
    pub fn register(&mut self, func: Arc<dyn SourceFunction>) {
        self.functions.insert(func.name().to_string(), func);
    }

    /// Get a source function by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn SourceFunction>> {
        self.functions.get(name).cloned()
    }

    /// Get all registered function names.
    pub fn function_names(&self) -> Vec<String> {
        self.functions.keys().cloned().collect()
    }
}

impl Default for SourceFunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_new() {
        let registry = SourceFunctionRegistry::new();
        assert!(registry.get("remote_dir").is_some());
        assert!(registry.get("web_scrape").is_some());
    }

    #[test]
    fn test_registry_get_remote_dir() {
        let registry = SourceFunctionRegistry::new();
        let func = registry.get("remote_dir").unwrap();
        assert_eq!(func.name(), "remote_dir");
    }

    #[test]
    fn test_registry_get_web_scrape() {
        let registry = SourceFunctionRegistry::new();
        let func = registry.get("web_scrape").unwrap();
        assert_eq!(func.name(), "web_scrape");
    }

    #[test]
    fn test_discovered_location_from_url() {
        let url = Url::parse("https://example.com/file.parquet").unwrap();
        let loc = DiscoveredLocation::from_url(url.clone());
        assert_eq!(loc.url, url);
        assert_eq!(loc.source_location, "https://example.com/file.parquet");
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
        let registry = SourceFunctionRegistry::new();
        let func = registry.get("remote_dir").unwrap();

        let mut args = HashMap::new();
        args.insert("url".to_string(), "file:///test/".to_string());
        args.insert("invalid_arg".to_string(), "value".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not accept argument 'invalid_arg'"));
        assert!(err.contains("Valid arguments:"));
        assert!(err.contains("url (required)"));
    }

    #[test]
    fn test_validate_args_missing_required() {
        let registry = SourceFunctionRegistry::new();
        let func = registry.get("remote_dir").unwrap();

        let args = HashMap::new(); // Missing required 'url'

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires a 'url' argument"));
        assert!(err.contains("Valid arguments:"));
    }

    #[test]
    fn test_validate_args_valid() {
        let registry = SourceFunctionRegistry::new();
        let func = registry.get("remote_dir").unwrap();

        let mut args = HashMap::new();
        args.insert("url".to_string(), "file:///test/".to_string());
        args.insert("patterns".to_string(), "*.parquet".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_ok());
    }
}
