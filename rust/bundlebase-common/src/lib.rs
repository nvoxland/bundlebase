#![deny(clippy::unwrap_used)]

pub mod arrow_types;
pub mod command_response;
pub mod content_address;
pub use content_address::{ContentAddress, ContentCategory, ContentFormat};
pub mod excel;
pub use command_response::{single_batch_stream, CommandResponse, OutputShape};
pub mod config;
pub mod connector;
pub mod data_reader;
pub mod indexed_blocks;
pub mod namespaced_name;
pub mod save_as;
pub mod source_utils;
pub use indexed_blocks::IndexedBlocks;
pub mod file_info;
pub mod io_dir;
pub mod io_file;
pub mod system_config;
pub mod versioned_blockid;

pub use versioned_blockid::VersionedBlockId;
pub mod object_id;
pub mod platform;
pub mod progress;
pub mod row_id;
pub mod versioning;

pub use config::{ConfigKey, ConfigProvider, ConfigScope, ConfigSource, Scope};
pub use namespaced_name::NamespacedName;
pub use object_id::{BlockId, ColumnId, ObjectId, ObjectIdAlias};
pub use platform::Platform;
pub use progress::{get_tracker, set_tracker, with_tracker, ProgressId, ProgressTracker};
pub use row_id::{boxed_rowid_stream, RowId, RowIdBatch, SendableRowIdBatchStream};

use std::error::Error;

/// Standard error type used throughout the Bundlebase codebase
pub type BundlebaseError = Box<dyn Error + Send + Sync>;

/// Returns the bundlebase format version as (major, minor), derived from Cargo.toml.
pub fn format_version() -> (u16, u16) {
    let version_str = env!("CARGO_PKG_VERSION");
    parse_format_version(version_str)
}

/// Returns the bundlebase format version as a "major.minor" string.
pub fn format_version_string() -> String {
    let (major, minor) = format_version();
    format!("{}.{}", major, minor)
}

/// Parse a "major.minor" or "major.minor.patch" version string into (major, minor).
pub fn parse_format_version(s: &str) -> (u16, u16) {
    let parts: Vec<&str> = s.split('.').collect();
    let major = parts.first().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor)
}

#[cfg(test)]
mod format_version_tests {
    use super::*;

    #[test]
    fn test_parse_format_version() {
        assert_eq!(parse_format_version("0.9.0"), (0, 9));
        assert_eq!(parse_format_version("1.2.3"), (1, 2));
        assert_eq!(parse_format_version("0.9"), (0, 9));
        assert_eq!(parse_format_version("2.0"), (2, 0));
    }

    #[test]
    fn test_format_version_from_cargo() {
        let (major, minor) = format_version();
        assert_eq!(major, 0);
        assert_eq!(minor, 9);
    }
}
