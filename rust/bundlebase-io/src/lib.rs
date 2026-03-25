#![deny(clippy::unwrap_used)]

//! IO module - Unified file and directory operations across multiple storage protocols.
//!
//! ## Module Structure
//!
//! **Generic (protocol-agnostic):**
//! - `file_info` - `FileInfo` struct for file metadata
//! - `file` - File traits: `IOReadFile`, `IOReadWriteFile`
//! - `dir` - Directory traits: `IOReadDir`, `IOReadWriteDir`
//! - `registry` - `IORegistry` for dispatching by URL scheme
//! - `util` - URL and path utilities
//!
//! **Protocol-specific (in `plugin/`):**
//! - `plugin::object_store` - file://, s3://, gs://, azure://, memory://, empty://
//! - `plugin::ftp` - ftp://
//! - `plugin::sftp` - sftp://
//! - `plugin::tar` - tar+file://

// Re-export common types for convenience
pub use bundlebase_common::{BundlebaseError, ConfigProvider};

// Generic modules
pub mod dir;
pub mod file;
pub mod file_info;
pub mod registry;
pub(crate) mod util;

// Plugin system with protocol-specific implementations
pub mod plugin;

#[cfg(test)]
pub(crate) mod test_utils;

// Re-export core types from registry
pub use registry::{io_registry, IOFactory, IORegistry};

// Re-export custom scheme registration (for benchmarks/tests)
pub use plugin::object_store::register_object_store_scheme;

// Re-export traits and types
pub use dir::{IOReadDir, IOReadWriteDir, WriteResult};
pub use file::{read_yaml, write_yaml, IOReadFile, IOReadWriteFile};
pub use file_info::FileInfo;

// Re-export ID types from common
pub use bundlebase_common::{BlockId, ObjectId, ObjectIdAlias};

use object_store::memory::InMemory;
use object_store::ObjectStore;
use std::sync::{Arc, OnceLock};
use url::Url;

pub static EMPTY_SCHEME: &str = "empty";
pub static EMPTY_URL: &str = "empty:///";

static MEMORY_STORE: OnceLock<Arc<InMemory>> = OnceLock::new();
static NULL_STORE: OnceLock<Arc<InMemory>> = OnceLock::new();

pub fn get_memory_store() -> Arc<InMemory> {
    MEMORY_STORE
        .get_or_init(|| Arc::new(InMemory::new()))
        .clone()
}

pub fn get_null_store() -> Arc<InMemory> {
    NULL_STORE.get_or_init(|| Arc::new(InMemory::new())).clone()
}

/// Create a writable directory from a URL.
pub async fn writable_dir_from_url(
    url: &Url,
    config: Arc<dyn ConfigProvider>,
) -> Result<Arc<dyn IOReadWriteDir>, BundlebaseError> {
    let dir = io_registry().create_writable_lister(url, config).await?;
    Ok(Arc::from(dir))
}

/// Create a writable directory from a URL string.
pub async fn writable_dir_from_str(
    url: &str,
    config: Arc<dyn ConfigProvider>,
) -> Result<Arc<dyn IOReadWriteDir>, BundlebaseError> {
    let parsed = plugin::object_store::str_to_url(url)?;
    writable_dir_from_url(&parsed, config).await
}

/// Create a writable directory from a URL using a pre-built object store.
pub fn writable_dir_with_store(
    url: &Url,
    store: Arc<dyn ObjectStore>,
    path: &object_store::path::Path,
    config: Arc<dyn ConfigProvider>,
) -> Result<Arc<dyn IOReadWriteDir>, BundlebaseError> {
    Ok(Arc::new(plugin::object_store::ObjectStoreDir::new(
        url, store, path, config,
    )?))
}

/// Create a readable file from a URL.
pub async fn readable_file_from_url(
    url: &Url,
    config: Arc<dyn ConfigProvider>,
) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
    io_registry().create_reader(url, config).await
}

/// Create a writable file from a URL.
pub async fn writable_file_from_url(
    url: &Url,
    config: Arc<dyn ConfigProvider>,
) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError> {
    io_registry().create_writer(url, config).await
}

/// Create a readable file from a path string.
pub async fn readable_file_from_path(
    path: &str,
    base: Arc<dyn IOReadDir>,
    config: Arc<dyn ConfigProvider>,
) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
    if path.contains(":") {
        readable_file_from_url(&Url::parse(path)?, config).await
    } else {
        base.file(path)
    }
}

/// Create a writable file from a path string.
pub async fn writable_file_from_path(
    path: &str,
    base: Arc<dyn IOReadWriteDir>,
    config: Arc<dyn ConfigProvider>,
) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError> {
    if path.contains(":") {
        writable_file_from_url(&Url::parse(path)?, config).await
    } else {
        base.writable_file(path)
    }
}

#[derive(Default)]
pub struct DataStorage {}

impl DataStorage {
    pub fn new() -> Self {
        Self {}
    }
}
