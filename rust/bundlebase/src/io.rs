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
//! - `plugin::tar` - tar://

// Generic modules
pub mod dir;
pub mod file;
pub mod file_info;
pub mod registry;
pub(crate) mod util;

// Plugin system with protocol-specific implementations
// Internal to crate - external code should use factory functions and trait objects
pub(crate) mod plugin;

// Re-export core types from registry
pub use registry::{io_registry, IOFactory, IORegistry};

// Re-export traits and types
pub use dir::{IOReadDir, IOReadWriteDir, WriteResult};
pub use file::{read_yaml, write_yaml, IOReadFile, IOReadWriteFile};
pub use file_info::FileInfo;

// Re-export ObjectId from data module
pub use crate::data::ObjectId;

use object_store::memory::InMemory;
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
///
/// This is the primary way to create directories outside the io module.
/// Returns an `Arc<dyn IOReadWriteDir>` that can be cloned cheaply.
pub fn writable_dir_from_url(
    url: &Url,
    config: Arc<crate::BundleConfig>,
) -> Result<Arc<dyn IOReadWriteDir>, crate::BundlebaseError> {
    Ok(Arc::new(plugin::object_store::ObjectStoreDir::from_url(
        url, config,
    )?))
}

/// Create a writable directory from a URL string.
///
/// Parses the URL string and creates a writable directory.
/// Relative paths are resolved against the current working directory.
pub fn writable_dir_from_str(
    url: &str,
    config: Arc<crate::BundleConfig>,
) -> Result<Arc<dyn IOReadWriteDir>, crate::BundlebaseError> {
    Ok(Arc::new(plugin::object_store::ObjectStoreDir::from_str(
        url, config,
    )?))
}

/// Create a readable file from a URL.
///
/// This is the primary way to create files for reading outside the io module.
pub fn readable_file_from_url(
    url: &Url,
    config: Arc<crate::BundleConfig>,
) -> Result<Box<dyn IOReadFile>, crate::BundlebaseError> {
    Ok(Box::new(plugin::object_store::ObjectStoreFile::from_url(
        url, config,
    )?))
}

/// Create a writable file from a URL.
///
/// This is the primary way to create files for writing outside the io module.
pub fn writable_file_from_url(
    url: &Url,
    config: Arc<crate::BundleConfig>,
) -> Result<Box<dyn IOReadWriteFile>, crate::BundlebaseError> {
    Ok(Box::new(plugin::object_store::ObjectStoreFile::from_url(
        url, config,
    )?))
}

/// Create a readable file from a path string.
///
/// If the path contains ":" it's treated as an absolute URL.
/// Otherwise, it's a relative path resolved against the base directory.
pub fn readable_file_from_path(
    path: &str,
    base: Arc<dyn IOReadDir>,
    config: Arc<crate::BundleConfig>,
) -> Result<Box<dyn IOReadFile>, crate::BundlebaseError> {
    if path.contains(":") {
        // Absolute URL
        readable_file_from_url(&Url::parse(path)?, config)
    } else {
        // Relative path - get file from base directory
        base.file(path)
    }
}

/// Create a writable file from a path string.
///
/// If the path contains ":" it's treated as an absolute URL.
/// Otherwise, it's a relative path resolved against the base directory.
pub fn writable_file_from_path(
    path: &str,
    base: Arc<dyn IOReadWriteDir>,
    config: Arc<crate::BundleConfig>,
) -> Result<Box<dyn IOReadWriteFile>, crate::BundlebaseError> {
    if path.contains(":") {
        // Absolute URL
        writable_file_from_url(&Url::parse(path)?, config)
    } else {
        // Relative path - get file from base directory
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
    use crate::io::plugin::object_store::ObjectStoreFile;
    use crate::BundleConfig;
    use url::Url;

    #[tokio::test]
    async fn memory_file() {
        // Verify file doesn't exist initially
        let url = &Url::parse("memory:///test_key").unwrap();
        let file = ObjectStoreFile::from_url(url, BundleConfig::default().into()).unwrap();
        assert!(!file.exists().await.unwrap());
        assert_eq!(true, file.version().await.is_err());

        // Write data and verify it's persisted
        file.write(bytes::Bytes::from("hello world")).await.unwrap();
        assert_eq!(
            bytes::Bytes::from("hello world"),
            file.read_bytes().await.unwrap().unwrap(),
            "Written data should be readable"
        );
        assert!(
            file.version().await.is_ok(),
            "Version should be available after write"
        );
    }

    #[tokio::test]
    async fn memory_file_multiple_writes() {
        // Test that multiple writes overwrite previous data
        let url = &Url::parse("memory:///multi_write_test").unwrap();
        let file = ObjectStoreFile::from_url(url, BundleConfig::default().into()).unwrap();

        // First write
        file.write(bytes::Bytes::from("first")).await.unwrap();
        assert_eq!(
            bytes::Bytes::from("first"),
            file.read_bytes().await.unwrap().unwrap()
        );

        // Second write (should overwrite)
        file.write(bytes::Bytes::from("much longer content"))
            .await
            .unwrap();
        assert_eq!(
            bytes::Bytes::from("much longer content"),
            file.read_bytes().await.unwrap().unwrap()
        );
    }

    #[tokio::test]
    async fn file_file() {
        // Absolute file path
        let url = &Url::parse("file:///absolute/path/file.txt").unwrap();
        let file = ObjectStoreFile::from_url(url, BundleConfig::default().into()).unwrap();
        assert_eq!(
            "file:///absolute/path/file.txt",
            file.url().to_string(),
            "Absolute file URL should be preserved"
        );

        // File URL from relative path
        let file = ObjectStoreFile::from_url(
            &Url::from_file_path(
                std::env::current_dir()
                    .unwrap()
                    .join("relative_path/file.txt"),
            )
            .unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        assert!(
            file.url().to_string().contains("relative_path/file.txt"),
            "Relative file path should be converted to URL"
        );

        // File URL from absolute path
        let file = ObjectStoreFile::from_url(
            &Url::from_file_path("/absolute/path/to/file.txt").unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        assert_eq!(
            "file:///absolute/path/to/file.txt",
            file.url().to_string(),
            "Absolute path should be converted correctly"
        );
    }

    #[tokio::test]
    async fn test_factory_rejects_unknown_scheme() {
        // Test that unknown URL schemes are rejected
        let url = &Url::parse("unknown://test").unwrap();
        let result = ObjectStoreFile::from_url(url, BundleConfig::default().into());
        assert!(result.is_err(), "Unknown scheme should be rejected");
        assert_eq!(
            result.err().unwrap().to_string(),
            "Generic URL error: Unable to recognise URL \"unknown://test\""
        );
    }

    #[tokio::test]
    async fn s3_file() {
        // Test S3 file URL handling
        let url = &Url::parse("s3://bucket/key").unwrap();
        let file = ObjectStoreFile::from_url(url, BundleConfig::default().into());
        assert!(file.is_ok(), "S3 URL should be supported");
        assert_eq!(
            "s3://bucket/key",
            file.unwrap().url().to_string(),
            "S3 URL should be preserved"
        );
    }

    #[tokio::test]
    async fn s3_file_various_paths() {
        // Test various S3 path formats
        let cases = vec![
            ("s3://my-bucket/file.txt", "s3://my-bucket/file.txt"),
            (
                "s3://bucket/path/to/file.parquet",
                "s3://bucket/path/to/file.parquet",
            ),
            (
                "s3://bucket/deep/nested/path/data.csv",
                "s3://bucket/deep/nested/path/data.csv",
            ),
        ];

        for (url_str, expected) in cases {
            let url = Url::parse(url_str).unwrap();
            let file = ObjectStoreFile::from_url(&url, BundleConfig::default().into()).unwrap();
            assert_eq!(expected, file.url().to_string());
        }
    }

    #[test]
    fn dir_from_url() {
        for (url, expected) in [
            ("memory:///test", "memory:///test"),
            ("memory:///test/", "memory:///test/"),
            ("memory:///test/here", "memory:///test/here"),
            ("memory:///test/here/", "memory:///test/here/"),
            ("file:///test/path", "file:///test/path"),
        ] {
            assert_eq!(
                expected,
                ObjectStoreFile::from_url(
                    &Url::parse(url).unwrap(),
                    BundleConfig::default().into()
                )
                .unwrap()
                .url()
                .to_string()
            );
        }
    }

    #[test]
    fn file_from_url() {
        for (url, expected) in [
            ("memory:///test", "memory:///test"),
            ("memory:///test/", "memory:///test/"),
            ("memory:///test/here.txt", "memory:///test/here.txt"),
            ("memory:///test/here/", "memory:///test/here/"),
            ("file:///test/path", "file:///test/path"),
        ] {
            assert_eq!(
                expected,
                ObjectStoreFile::from_url(
                    &Url::parse(url).unwrap(),
                    BundleConfig::default().into()
                )
                .unwrap()
                .url()
                .to_string()
            );
        }
    }
}
