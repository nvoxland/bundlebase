//! Core IO traits for unified file and directory operations across multiple protocols.
//!
//! This module defines the trait hierarchy for reading, writing, and listing files
//! regardless of the underlying storage protocol (local, cloud, FTP, SFTP, tar, etc.).

use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;
use rand::Rng;
use sha2::{Digest, Sha256};
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
    /// Returns None if the URL has no path segments or the last segment is empty.
    pub fn filename(&self) -> Option<&str> {
        self.url
            .path_segments()
            .and_then(|segments| segments.last())
            .filter(|s| !s.is_empty())
    }
}

/// Read-only file operations.
/// Implemented by all storage backends - both read-only sources (FTP) and read-write stores.
#[async_trait]
pub trait IOReadFile: Send + Sync + Debug {
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
/// Separated from IOReadFile because not all file references support directory operations.
#[async_trait]
pub trait IODir: Send + Sync + Debug {
    /// Returns the URL this directory represents.
    fn url(&self) -> &Url;

    /// List all files in this directory.
    async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError>;

    /// Get a subdirectory reference.
    /// The subdirectory is not validated to exist.
    fn subdir(&self, name: &str) -> Result<Box<dyn IODir>, BundlebaseError>;

    /// Get a file reference within this directory.
    /// The file is not validated to exist.
    fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError>;

    /// Get a writable file reference within this directory.
    /// Returns an error for read-only directories.
    fn writable_file(&self, _name: &str) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError> {
        Err(format!("Directory {} does not support writable files", self.url()).into())
    }

    /// Rename a file within this directory.
    /// Returns an error for read-only directories.
    async fn rename(&self, _from: &str, _to: &str) -> Result<(), BundlebaseError> {
        Err(format!("Directory {} does not support rename", self.url()).into())
    }

    /// Write data stream to a new file named by its content hash.
    ///
    /// The stream is consumed while computing a SHA256 hash. The file is written
    /// to a temporary location first, then renamed to a content-addressed name.
    /// If a file with that hash already exists, the temp file is deleted
    /// and the existing filename is returned (deduplication).
    ///
    /// Returns the relative filename in format `{hash_prefix}_{suffix}`.
    async fn write_content_addressed(
        &self,
        mut source: BoxStream<'static, Result<Bytes, std::io::Error>>,
        suffix: &str,
    ) -> Result<String, BundlebaseError> {
        use futures::StreamExt;

        let temp_name = format!("temp_{:016x}", rand::rng().random::<u64>());
        let temp_file = self.writable_file(&temp_name)?;

        // Consume stream: compute hash while buffering
        let mut hasher = Sha256::new();
        let mut buffer = Vec::new();
        while let Some(chunk_result) = source.next().await {
            let chunk = chunk_result.map_err(|e| BundlebaseError::from(e.to_string()))?;
            hasher.update(&chunk);
            buffer.extend_from_slice(&chunk);
        }

        // Write buffered data to temp file
        temp_file.write(Bytes::from(buffer)).await?;

        // Compute final name from hash (16 char hash prefix for readability)
        let hash = format!("{:x}", hasher.finalize());
        let final_name = format!("{}_{}", &hash[..16], suffix);

        // Check for duplicate - rename or delete temp
        if self.file(&final_name)?.exists().await? {
            temp_file.delete().await?;
        } else {
            self.rename(&temp_name, &final_name).await?;
        }

        Ok(final_name)
    }
}

/// Write operations for storage backends that support modification.
/// Not implemented by read-only backends (FTP, SCP when used as sources).
#[async_trait]
pub trait IOReadWriteFile: IOReadFile {
    /// Write bytes to file, overwriting if exists.
    async fn write(&self, data: Bytes) -> Result<(), BundlebaseError>;

    /// Write stream to file, overwriting if exists.
    /// Uses a boxed stream for dyn compatibility.
    async fn write_stream(
        &self,
        source: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Result<(), BundlebaseError>;

    /// Delete the file.
    /// Returns Ok even if the file doesn't exist.
    async fn delete(&self) -> Result<(), BundlebaseError>;
}

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

    #[test]
    fn test_file_info_filename_with_valid_filename() {
        let url = Url::parse("memory:///path/to/file.txt").unwrap();
        let info = FileInfo::new(url);
        assert_eq!(info.filename(), Some("file.txt"));
    }

    #[test]
    fn test_file_info_filename_root_path() {
        let url = Url::parse("memory:///file.txt").unwrap();
        let info = FileInfo::new(url);
        assert_eq!(info.filename(), Some("file.txt"));
    }

    #[test]
    fn test_file_info_filename_trailing_slash() {
        // Trailing slash means the last segment is empty
        let url = Url::parse("memory:///path/to/").unwrap();
        let info = FileInfo::new(url);
        // Returns None because last segment is empty
        assert_eq!(info.filename(), None);
    }

    #[test]
    fn test_file_info_filename_empty_path() {
        // URL with empty path
        let url = Url::parse("memory:///").unwrap();
        let info = FileInfo::new(url);
        assert_eq!(info.filename(), None);
    }
}
