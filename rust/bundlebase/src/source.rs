//! Source module for connector definitions, data discovery, and fetch orchestration.

pub(crate) mod connector;
pub(crate) mod connector_utils;
pub(crate) mod fetch;
pub(crate) mod shared_utils;
pub(crate) mod source_utils;

use crate::BundlebaseError;

/// Sync mode for source fetch operations.
///
/// Controls how fetch handles existing files when checking for updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Only add new files
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

impl std::fmt::Display for SyncMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncMode::Add => write!(f, "ADD"),
            SyncMode::Update => write!(f, "UPDATE"),
            SyncMode::Sync => write!(f, "SYNC"),
        }
    }
}

pub use connector::plugin::kaggle::KaggleConnector;
pub use connector::plugin::native;
pub use connector::plugin::PostgresConnector;
pub use connector::plugin::RemoteDirConnector;
pub use connector::plugin::WebScrapeConnector;
pub use connector::{
    format_fetch_summary, validate_connector_args, ArgSpec, AttachedFileInfo, DiscoveredLocation,
    FetchAction, FetchedBlock, FetchResults, MaterializedData, SourceData, Connector,
    ConnectorRegistry, ConnectorSignature,
};
pub use fetch::orchestrate_fetch;
