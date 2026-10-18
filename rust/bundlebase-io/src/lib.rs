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

/// Create a read-only directory from a URL string.
pub async fn readable_dir_from_str(
    url: &str,
    config: Arc<dyn ConfigProvider>,
) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
    let parsed = plugin::object_store::str_to_url(url)?;
    io_registry().create_lister(&parsed, config).await
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
/// Absolute filesystem paths are local files, regardless of the base protocol.
/// Relative paths resolve within the base directory; explicit URLs keep their protocol.
pub async fn readable_file_from_path(
    path: &str,
    base: Arc<dyn IOReadDir>,
    config: Arc<dyn ConfigProvider>,
) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
    if std::path::Path::new(path).is_absolute() {
        let url = Url::from_file_path(path)
            .map_err(|_| BundlebaseError::from(format!("Invalid file path: {}", path)))?;
        readable_file_from_url(&url, config).await
    } else if path.contains(":") {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{random_memory_dir, test_config};

    #[tokio::test]
    async fn absolute_file_paths_ignore_base_and_preserve_special_characters(
    ) -> Result<(), BundlebaseError> {
        let temp = tempfile::tempdir()?;
        let local_base_url =
            Url::from_directory_path(temp.path().join("bundle")).expect("absolute directory URL");
        let local_base = writable_dir_from_url(&local_base_url, test_config()).await?;
        let memory_base = random_memory_dir();
        let mut names = vec!["sales.csv", "sales 100% #é.csv"];
        if !cfg!(windows) {
            names.push("sales:2026.csv");
        }
        for name in names {
            let path = temp.path().join(name);
            std::fs::write(&path, "id\n1\n")?;
            let expected = Url::from_file_path(&path).expect("absolute file URL");
            for base in [local_base.clone(), memory_base.clone()] {
                let file = readable_file_from_path(
                    path.to_str().expect("UTF-8 test path"),
                    base.clone(),
                    test_config(),
                )
                .await?;
                assert_eq!(file.url(), &expected);
                assert_eq!(file.read_str().await?.as_deref(), Some("id\n1\n"));
                // File readers construct ObjectStoreFile directly during format detection.
                let object_file = plugin::object_store::ObjectStoreFile::from_str(
                    path.to_str().expect("UTF-8 test path"),
                    base.as_ref(),
                    test_config(),
                )?;
                assert_eq!(object_file.url(), &expected);
                assert_eq!(object_file.read_str().await?.as_deref(), Some("id\n1\n"));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn relative_paths_and_explicit_urls_keep_their_resolution() -> Result<(), BundlebaseError>
    {
        let base = random_memory_dir();
        base.writable_file("nested/sales.csv")?
            .write(bytes::Bytes::from_static(b"id\n1\n"))
            .await?;
        let relative =
            readable_file_from_path("nested/sales.csv", base.clone(), test_config()).await?;
        let expected = base.file("nested/sales.csv")?;
        assert_eq!(relative.url(), expected.url());
        assert_eq!(relative.read_str().await?.as_deref(), Some("id\n1\n"));

        let explicit =
            readable_file_from_path(expected.url().as_str(), random_memory_dir(), test_config())
                .await?;
        assert_eq!(explicit.url(), expected.url());
        assert_eq!(explicit.read_str().await?.as_deref(), Some("id\n1\n"));

        let temp = tempfile::tempdir()?;
        let path = temp.path().join("sales 100%.csv");
        std::fs::write(&path, "id\n2\n")?;
        let url = Url::from_file_path(path).expect("absolute file URL");
        let explicit_local = readable_file_from_path(url.as_str(), base, test_config()).await?;
        assert_eq!(explicit_local.url(), &url);
        assert_eq!(explicit_local.read_str().await?.as_deref(), Some("id\n2\n"));
        Ok(())
    }
}
