//! Core IO traits for unified file and directory operations across multiple protocols.
//!
//! This module defines the trait hierarchy for reading, writing, and listing files
//! regardless of the underlying storage protocol (local, cloud, FTP, SFTP, tar, etc.).

use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;
use std::fmt::Debug;
use url::Url;

/// Information about a file in storage.
/// Protocol-agnostic metadata common to all storage backends.
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// Full URL of the file
    pub url: Url,
    /// File size in bytes (if known)
    pub size: Option<u64>,
    /// Last modified time (if available)
    pub modified: Option<DateTime<Utc>>,
}

impl FileInfo {
    /// Create a new FileInfo with the given URL.
    pub fn new(url: Url) -> Self {
        Self {
            url,
            size: None,
            modified: None,
        }
    }

    /// Create a FileInfo with size information.
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Create a FileInfo with modification time.
    pub fn with_modified(mut self, modified: DateTime<Utc>) -> Self {
        self.modified = Some(modified);
        self
    }

    /// Get the filename portion of the URL path.
    pub fn filename(&self) -> &str {
        self.url
            .path_segments()
            .and_then(|segments| segments.last())
            .unwrap_or("")
    }
}

/// Read-only file operations.
/// Implemented by all storage backends - both read-only sources (FTP) and read-write stores.
#[async_trait]
pub trait IOReader: Send + Sync + Debug {
    /// Returns the URL this reader represents.
    fn url(&self) -> &Url;

    /// Check if a file exists at this location.
    async fn exists(&self) -> Result<bool, BundlebaseError>;

    /// Read file contents as bytes (for small files).
    /// Returns `None` if the file doesn't exist.
    async fn read_bytes(&self) -> Result<Option<Bytes>, BundlebaseError>;

    /// Read file contents as a stream (for large files).
    /// Returns `None` if the file doesn't exist.
    async fn read_stream(
        &self,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, BundlebaseError>>>, BundlebaseError>;

    /// Get file metadata.
    /// Returns `None` if the file doesn't exist.
    async fn metadata(&self) -> Result<Option<FileInfo>, BundlebaseError>;

    /// Read file contents as a UTF-8 string.
    /// Returns `None` if the file doesn't exist.
    async fn read_str(&self) -> Result<Option<String>, BundlebaseError> {
        match self.read_bytes().await? {
            Some(bytes) => Ok(Some(String::from_utf8(bytes.to_vec())?)),
            None => Ok(None),
        }
    }

    /// Returns a version identifier for the file.
    /// This could be an ETag, last modified time hash, or version ID.
    async fn version(&self) -> Result<String, BundlebaseError>;
}

/// Directory listing operations.
/// Separated from IOReader because not all file references support directory operations.
#[async_trait]
pub trait IOLister: Send + Sync + Debug {
    /// Returns the URL this lister represents.
    fn url(&self) -> &Url;

    /// List all files in this directory.
    async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError>;

    /// Get a subdirectory reference.
    /// The subdirectory is not validated to exist.
    fn subdir(&self, name: &str) -> Result<Box<dyn IOLister>, BundlebaseError>;

    /// Get a file reference within this directory.
    /// The file is not validated to exist.
    fn file(&self, name: &str) -> Result<Box<dyn IOReader>, BundlebaseError>;
}

/// Write operations for storage backends that support modification.
/// Not implemented by read-only backends (FTP, SCP when used as sources).
#[async_trait]
pub trait IOWriter: IOReader {
    /// Write bytes to file, overwriting if exists.
    async fn write(&self, data: Bytes) -> Result<(), BundlebaseError>;

    /// Write stream to file, overwriting if exists.
    /// Uses a boxed stream for dyn compatibility.
    async fn write_stream_boxed(
        &self,
        source: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Result<(), BundlebaseError>;

    /// Delete the file.
    /// Returns Ok even if the file doesn't exist.
    async fn delete(&self) -> Result<(), BundlebaseError>;
}

/// Combined trait for backends that support both reading and writing.
pub trait IOReadWrite: IOReader + IOWriter {}

/// Blanket implementation for types that implement both IOReader and IOWriter.
impl<T: IOReader + IOWriter> IOReadWrite for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_info_builder() {
        let url = Url::parse("memory:///test.txt").unwrap();
        let info = FileInfo::new(url.clone())
            .with_size(1024)
            .with_modified(Utc::now());

        assert_eq!(info.url, url);
        assert_eq!(info.size, Some(1024));
        assert!(info.modified.is_some());
    }
}
