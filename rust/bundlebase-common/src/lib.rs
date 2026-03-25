#![deny(clippy::unwrap_used)]

pub mod arrow_types;
pub mod command_response;
pub use command_response::{CommandResponse, OutputShape, single_batch_stream};
pub mod config;
pub mod connector;
pub mod namespaced_name;
pub mod source_utils;
pub mod data_reader;
pub mod indexed_blocks;
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

pub use config::{
    ConfigKey, ConfigProvider, ConfigScope, ConfigSource, Scope,
};
pub use namespaced_name::NamespacedName;
pub use object_id::{BlockId, ColumnId, ObjectId, ObjectIdAlias};
pub use platform::Platform;
pub use progress::{get_tracker, set_tracker, with_tracker, ProgressId, ProgressTracker};
pub use row_id::{boxed_rowid_stream, RowId, RowIdBatch, SendableRowIdBatchStream};

use std::error::Error;

/// Standard error type used throughout the Bundlebase codebase
pub type BundlebaseError = Box<dyn Error + Send + Sync>;
