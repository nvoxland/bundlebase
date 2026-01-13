//! Directory IO traits for reading and writing directories.

use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use rand::Rng;
use sha2::{Digest, Sha256};
use std::fmt::Debug;
use url::Url;

use super::{FileInfo, IOReadFile, IOReadWriteFile};

/// Read-only directory operations.
/// Implemented by all storage backends - both read-only sources (FTP) and read-write stores.
#[async_trait]
pub trait IOReadDir: Send + Sync + Debug {
    /// Returns the URL this directory represents.
    fn url(&self) -> &Url;

    /// List all files in this directory.
    async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError>;

    /// Get a subdirectory reference.
    /// The subdirectory is not validated to exist.
    fn subdir(&self, name: &str) -> Result<Box<dyn IOReadDir>, BundlebaseError>;

    /// Get a file reference within this directory.
    /// The file is not validated to exist.
    fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError>;
}

/// Write operations for directories that support modification.
/// Not implemented by read-only backends (FTP, SCP when used as sources).
#[async_trait]
pub trait IOReadWriteDir: IOReadDir {
    /// Get a writable subdirectory reference.
    /// The subdirectory is not validated to exist.
    fn writable_subdir(&self, name: &str) -> Result<Box<dyn IOReadWriteDir>, BundlebaseError>;

    /// Get a writable file reference within this directory.
    fn writable_file(&self, name: &str) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError>;

    /// Rename a file within this directory.
    async fn rename(&self, from: &str, to: &str) -> Result<(), BundlebaseError>;

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
