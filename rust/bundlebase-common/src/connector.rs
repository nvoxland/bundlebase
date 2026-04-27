//! Connector trait and associated types for data source discovery and fetching.
//!
//! This module defines the `Connector` trait and its supporting types. Trait
//! definitions live here in `bundlebase-common` so that connector implementations
//! can live in a separate crate without circular dependencies on the core.

use crate::{BundlebaseError, ConfigProvider};
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
    pub fn from_batch(batch: RecordBatch) -> Self {
        SourceData::Arrow(Box::pin(stream::once(async { Ok(batch) })))
    }

    /// Create a RawBytes variant from in-memory bytes.
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

/// Data format describing what a connector produces.
///
/// Used on `DiscoveredLocation` to identify the input format.
/// The `SaveStrategy` on the source determines how this data gets saved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceFormat {
    Csv,
    Tsv,
    /// Regular JSON (array of objects). Must be converted to Parquet.
    Json,
    /// JSON Lines (one JSON object per line). Can be attached directly.
    JsonL,
    Parquet,
    Xlsx,
    Xls,
    Ods,
    /// Format not yet determined — will be detected from content after download.
    Auto,
}

impl SourceFormat {
    /// Parse a format string (file extension or name) into a SourceFormat.
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "csv" => SourceFormat::Csv,
            "tsv" => SourceFormat::Tsv,
            "json" => SourceFormat::Json,
            "jsonl" => SourceFormat::JsonL,
            "parquet" => SourceFormat::Parquet,
            "xlsx" => SourceFormat::Xlsx,
            "xls" => SourceFormat::Xls,
            "ods" => SourceFormat::Ods,
            "auto" | _ => SourceFormat::Auto,
        }
    }

    /// File extension for this format (without leading dot).
    pub fn extension(&self) -> &'static str {
        match self {
            SourceFormat::Csv => "csv",
            SourceFormat::Tsv => "tsv",
            SourceFormat::Json => "json",
            SourceFormat::JsonL => "jsonl",
            SourceFormat::Parquet => "parquet",
            SourceFormat::Xlsx => "xlsx",
            SourceFormat::Xls => "xls",
            SourceFormat::Ods => "ods",
            SourceFormat::Auto => "dat",
        }
    }
}

impl std::fmt::Display for SourceFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.extension())
    }
}

/// A partition of source data discovered during the discovery phase.
///
/// Each `DiscoveredLocation` becomes one block in the bundle.
#[derive(Debug, Clone)]
pub struct DiscoveredLocation {
    /// Stable identifier for this partition.
    pub location: String,
    /// Whether this location requires copying data into the bundle's data directory.
    pub must_copy: bool,
    /// Input data format describing what the connector produces.
    /// This is the format of the data, not how it will be saved — save policy is on the Source.
    pub format: SourceFormat,
    /// Source-specific version string used for change detection during sync.
    pub version: String,
    /// Optional row count, declared by the connector when known cheaply.
    ///
    /// Parquet readers and any connector with an authoritative row-count manifest
    /// should populate this. JSONL / CSV / connectors that would have to fully
    /// parse the data to count rows should leave it `None`.
    ///
    /// Used by `FETCH ... DRY RUN` to report the expected row delta without
    /// actually reading the data. When `None`, dry-run output reports the row
    /// delta as estimated (the contribution from this location is skipped).
    pub num_rows: Option<u64>,
}

/// Result of materializing a single data unit from a source.
#[derive(Debug, Clone)]
pub struct MaterializedData {
    /// Location of the materialized file
    pub attach_location: String,
    /// Original source location identifier
    pub source_location: String,
    /// Full URL to the source file
    pub source_url: String,
    /// SHA256 hash of the content. None if not yet known.
    pub hash: Option<String>,
    /// Source-specific version string
    pub version: String,
    /// Optional row count, propagated from `DiscoveredLocation::num_rows`.
    /// Used by FETCH DRY RUN to estimate row deltas without reading the data.
    pub num_rows: Option<u64>,
}

/// Metadata about an attached file from a source.
#[derive(Debug, Clone)]
pub struct AttachedFileInfo {
    /// The location where this block is currently stored
    pub location: String,
    /// Version string from AttachBlockOp
    pub version: String,
    /// File size in bytes
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

/// Per-source-location record produced by a fetch.
///
/// Note: this is *not* one record per bundle block. When `MIN BATCH` merges
/// multiple connector locations into a single batch block, every original
/// source location still gets its own `FetchedSource` entry, all pointing at
/// the same `attach_location`.
#[derive(Debug, Clone)]
pub struct FetchedSource {
    /// On-disk path of the bundle block this source location was attached to.
    /// Multiple `FetchedSource` entries can share the same `attach_location`
    /// when batching merged them into one block.
    pub attach_location: String,
    /// The connector-reported source location identifier.
    pub source_location: String,
    /// Source-specific version string
    pub version: String,
    /// Optional row count declared by the connector (None = unknown).
    pub num_rows: Option<u64>,
}

/// Per-source-location record for content that was removed from the bundle.
#[derive(Debug, Clone)]
pub struct RemovedSource {
    /// The connector-reported source location identifier that was removed.
    pub source_location: String,
    /// Source-specific version string of the removed source.
    pub version: String,
    /// Row count from the removed source's stored metadata, when known.
    pub num_rows: Option<u64>,
}

/// Results from fetching a single source.
#[derive(Debug, Clone)]
pub struct FetchResults {
    /// Connector name
    pub connector: String,
    /// Stable identifier for this source — users can pass it to
    /// `DESCRIBE SOURCE` to get full configuration.
    pub source_id: crate::object_id::ObjectId,
    /// Pack name
    pub pack: String,
    /// Blocks that were newly added
    pub added: Vec<FetchedSource>,
    /// Blocks that were replaced
    pub replaced: Vec<FetchedSource>,
    /// Blocks that were removed
    pub removed: Vec<RemovedSource>,
    /// Total rows attached to this source before the fetch. `None` means
    /// "unknown" (e.g. some block lacked num_rows metadata) — display as
    /// blank rather than 0 to keep the distinction.
    pub rows_before: Option<u64>,
    /// Total rows attached to this source after the fetch. `None` when any
    /// pending Add/Replace had `num_rows = None` from the connector — the
    /// estimate would otherwise silently understate the delta.
    pub rows_after: Option<u64>,
}

impl FetchResults {
    /// Create empty results.
    pub fn empty(
        connector: String,
        source_id: crate::object_id::ObjectId,
        pack: String,
    ) -> Self {
        Self {
            connector,
            source_id,
            pack,
            added: Vec::new(),
            replaced: Vec::new(),
            removed: Vec::new(),
            rows_before: Some(0),
            rows_after: Some(0),
        }
    }

    /// Create FetchResults from a list of FetchActions.
    ///
    /// Removed blocks need extra metadata (version, num_rows) that the caller
    /// must look up against the bundle — pass them as `removed_metadata`,
    /// keyed by source_location. Lookups that miss leave version blank and
    /// num_rows None.
    pub fn from_actions(
        connector: String,
        source_id: crate::object_id::ObjectId,
        pack: String,
        actions: Vec<FetchAction>,
        removed_metadata: &std::collections::HashMap<String, (String, Option<u64>)>,
    ) -> Self {
        let mut added = Vec::new();
        let mut replaced = Vec::new();
        let mut removed = Vec::new();

        for action in actions {
            match action {
                FetchAction::Add(data) => {
                    added.push(FetchedSource {
                        attach_location: data.attach_location,
                        source_location: data.source_location,
                        version: data.version,
                        num_rows: data.num_rows,
                    });
                }
                FetchAction::Replace { data, .. } => {
                    replaced.push(FetchedSource {
                        attach_location: data.attach_location,
                        source_location: data.source_location,
                        version: data.version,
                        num_rows: data.num_rows,
                    });
                }
                FetchAction::Remove { source_location } => {
                    let (version, num_rows) = removed_metadata
                        .get(&source_location)
                        .cloned()
                        .unwrap_or_default();
                    removed.push(RemovedSource {
                        source_location,
                        version,
                        num_rows,
                    });
                }
            }
        }

        Self {
            connector,
            source_id,
            pack,
            added,
            replaced,
            removed,
            rows_before: Some(0),
            rows_after: Some(0),
        }
    }

    /// Total number of actions.
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
#[async_trait]
pub trait Connector: Send + Sync {
    /// Return the signature (name + arg specs) for this connector.
    fn signature(&self) -> ConnectorSignature;

    /// Custom validation for arguments. Default does nothing.
    fn validate_args(&self, _args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        Ok(())
    }

    /// Discover all data partitions from the source.
    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        config: &Arc<dyn ConfigProvider>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError>;

    /// Return raw bytes for a discovered location.
    async fn data(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<SourceData>, BundlebaseError>;

    /// Return a stable downloadable URL for a discovered location.
    async fn stable_url(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<Url>, BundlebaseError>;
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

/// Validate connector arguments against the signature specs.
pub fn validate_connector_args(
    args: &HashMap<String, String>,
    sig: &ConnectorSignature,
) -> Result<(), BundlebaseError> {
    let specs = &sig.arg_specs;

    // Check for required arguments
    for spec in specs {
        if spec.required && !args.contains_key(spec.name) {
            let valid_args = format_arg_list(specs);
            return Err(format!(
                "Function '{}' requires a '{}' argument. Valid arguments: {}",
                sig.name, spec.name, valid_args
            )
            .into());
        }
    }

    // Check for unknown arguments (skip if the source accepts extra args).
    // Keys prefixed with "_" are reserved system keys and always allowed.
    // Keys prefixed with "json_" are reader-level options handled by the data pipeline, not connectors.
    if !sig.accepts_extra_args {
        let valid_names: HashSet<&str> = specs.iter().map(|s| s.name).collect();
        for key in args.keys() {
            if !key.starts_with('_')
                && !key.starts_with("json_")
                && !valid_names.contains(key.as_str())
            {
                let valid_args = format_arg_list(specs);
                return Err(format!(
                    "Function '{}' does not accept argument '{}'. Valid arguments: {}",
                    sig.name, key, valid_args
                )
                .into());
            }
        }
    }

    Ok(())
}
