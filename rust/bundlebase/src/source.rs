//! Source module for source function definitions and discovery.

mod ipc;
pub(crate) mod kaggle;
mod postgres;
mod remote_dir;
mod source_function;
pub(crate) mod source_utils;
mod web_scrape;

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

pub use kaggle::KaggleSource;
pub use postgres::PostgresFunction;
pub use remote_dir::RemoteDirFunction;
pub use source_function::{
    format_fetch_summary, validate_source_args, ArgSpec, AttachedFileInfo, DiscoveredLocation,
    FetchAction, FetchedBlock, FetchResults, MaterializedData, SourceData, SourceFunction,
    SourceFunctionRegistry, SourceFunctionSignature,
};
pub use source_utils::orchestrate_fetch;
pub use web_scrape::WebScrapeFunction;
