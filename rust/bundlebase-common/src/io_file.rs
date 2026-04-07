//! File IO traits for reading and writing files.

use crate::BundlebaseError;
use crate::progress::ProgressScope;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::fmt::Debug;
use url::Url;

use crate::file_info::FileInfo;

/// Read-only file operations.
/// Implemented by all storage backends - both read-only sources (FTP) and read-write stores.
#[async_trait]
pub trait IOReadFile: Send + Sync + Debug {
    /// Returns the URL this reader represents.
    fn url(&self) -> &Url;

    /// Check if a file exists at this location.
    async fn exists(&self) -> Result<bool, BundlebaseError>;

    /// Open file contents as a raw byte stream (for large files).
    /// Returns `None` if the file doesn't exist.
    /// Implementors provide the raw stream; callers should prefer `read_stream` for progress tracking.
    async fn open_stream(
        &self,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, BundlebaseError>>>, BundlebaseError>;

    /// Get file metadata.
    /// Returns `None` if the file doesn't exist.
    async fn metadata(&self) -> Result<Option<FileInfo>, BundlebaseError>;

    /// Read file contents as a stream, reporting progress via the active `ProgressTracker`.
    ///
    /// Wraps `open_stream()` with a progress scope so byte throughput is visible during
    /// S3, GCS, SFTP, FTP reads. Callers receive the same chunk-by-chunk stream but each
    /// chunk increments the registered tracker.
    async fn read_stream(
        &self,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, BundlebaseError>>>, BundlebaseError> {
        let Some(raw) = self.open_stream().await? else {
            return Ok(None);
        };
        let progress = ProgressScope::new(&format!("Reading {}", self.url()), None);
        let tracked = raw.map(move |chunk_result| {
            if let Ok(ref chunk) = chunk_result {
                progress.increment(chunk.len() as u64, None);
            }
            chunk_result
        });
        Ok(Some(Box::pin(tracked)))
    }

    /// Read file contents as bytes.
    /// Returns `None` if the file doesn't exist.
    /// Progress is reported via the active `ProgressTracker` as chunks stream in.
    async fn read_bytes(&self) -> Result<Option<Bytes>, BundlebaseError> {
        let Some(mut stream) = self.read_stream().await? else {
            return Ok(None);
        };
        let mut buffer = Vec::new();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }
        Ok(Some(Bytes::from(buffer)))
    }

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

    /// Compute the SHA256 hash of the file content by streaming.
    ///
    /// Reads the file in chunks and computes the hash incrementally,
    /// avoiding loading the entire file into memory.
    /// Returns the full 64-character hex string.
    async fn compute_hash(&self) -> Result<String, BundlebaseError> {
        let mut hasher = Sha256::new();

        if let Some(mut stream) = self.open_stream().await? {
            while let Some(chunk_result) = stream.next().await {
                let chunk = chunk_result?;
                hasher.update(&chunk);
            }
        } else {
            return Err(BundlebaseError::from(format!(
                "File not found: {}",
                self.url()
            )));
        }

        Ok(format!("{:x}", hasher.finalize()))
    }
}

/// Write operations for storage backends that support modification.
/// Not implemented by read-only backends (FTP, SFTP when used as sources).
#[async_trait]
pub trait IOReadWriteFile: IOReadFile {
    /// Write bytes to file, overwriting if exists.
    async fn write(&self, data: Bytes) -> Result<(), BundlebaseError>;

    /// Write stream to file, overwriting if exists.
    /// Uses a boxed stream for dyn compatibility.
    async fn write_stream(
        &self,
        source: BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Result<(), BundlebaseError>;

    /// Delete the file.
    /// Returns Ok even if the file doesn't exist.
    async fn delete(&self) -> Result<(), BundlebaseError>;
}

