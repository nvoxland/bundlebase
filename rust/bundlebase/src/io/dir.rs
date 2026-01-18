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

/// Result of writing a stream to a content-addressed file.
/// Contains both the file reference and the computed SHA256 hash.
#[derive(Debug)]
pub struct WriteResult {
    /// Reference to the written file
    pub file: Box<dyn IOReadFile>,
    /// SHA256 hash of the content (full 64-character hex string)
    pub hash: String,
}

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

    /// Get the relative path of a file within this directory.
    ///
    /// Returns the path of the file relative to this directory's URL.
    /// Returns an error if the file is not within this directory.
    ///
    /// # Example
    /// If the directory URL is `file:///data/` and the file URL is
    /// `file:///data/ab/cdef.parquet`, returns `"ab/cdef.parquet"`.
    fn relative_path(&self, file: &dyn IOReadFile) -> Result<String, BundlebaseError> {
        let dir_url = self.url();
        let file_url = file.url();

        // Check that schemes and hosts match
        if dir_url.scheme() != file_url.scheme() {
            return Err(format!(
                "File scheme '{}' does not match directory scheme '{}'",
                file_url.scheme(),
                dir_url.scheme()
            )
            .into());
        }

        if dir_url.host() != file_url.host() {
            return Err(format!(
                "File host '{:?}' does not match directory host '{:?}'",
                file_url.host(),
                dir_url.host()
            )
            .into());
        }

        // Normalize directory path to ensure it ends with /
        let dir_path = dir_url.path();
        let dir_path_normalized = if dir_path.ends_with('/') {
            dir_path.to_string()
        } else {
            format!("{}/", dir_path)
        };

        let file_path = file_url.path();

        // Check that the file path starts with the directory path
        if let Some(relative) = file_path.strip_prefix(&dir_path_normalized) {
            Ok(relative.to_string())
        } else {
            Err(format!(
                "File '{}' is not within directory '{}'",
                file_url, dir_url
            )
            .into())
        }
    }
}

/// Write operations for directories that support modification.
/// Not implemented by read-only backends (FTP, SFTP when used as sources).
#[async_trait]
pub trait IOReadWriteDir: IOReadDir {
    /// Check if this directory uses local filesystem storage.
    /// Used to determine optimal temp directory location for index building.
    ///
    /// Returns `true` for local storage backends (file://, tar://, memory://)
    /// and `false` for remote storage backends (s3://, gs://, azure://).
    fn is_local_storage(&self) -> bool {
        matches!(self.url().scheme(), "file" | "tar" | "memory")
    }

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
    /// to a temporary location in a `temp/` subdirectory first, then moved to
    /// a content-addressed location. If a file with that hash already exists,
    /// the temp file is deleted and the existing file is returned (deduplication).
    ///
    /// Files are organized by hash prefix: the first 2 hex characters become
    /// a subdirectory, and the remaining hash characters become the filename.
    ///
    /// Returns a `WriteResult` containing the file reference and the computed hash.
    async fn write_stream(
        &self,
        mut source: BoxStream<'static, Result<Bytes, std::io::Error>>,
        ext: &str,
    ) -> Result<WriteResult, BundlebaseError> {
        use futures::StreamExt;

        // Create temp subdir and write temp file there
        let temp_dir = self.writable_subdir("temp")?;
        let temp_name = format!("tohash_{:016x}", rand::rng().random::<u64>());
        let temp_file = temp_dir.writable_file(&temp_name)?;

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

        // Compute hash - first 2 chars = subdir, remaining chars = filename
        let hash = format!("{:x}", hasher.finalize());
        let subdir_name = &hash[..2];
        let file_name = format!("{}.{}", &hash[2..16], ext);

        // Get or create the hash-prefix subdirectory
        let final_dir = self.writable_subdir(subdir_name)?;

        // Check for duplicate - delete temp or move to final location
        if final_dir.file(&file_name)?.exists().await? {
            // Best-effort delete - some backends (like tar) don't support deletion
            let _ = temp_file.delete().await;
        } else {
            // Move from temp dir to final location
            // Read+write+delete since rename across directories may not work
            let final_file = final_dir.writable_file(&file_name)?;
            let temp_bytes = temp_file
                .read_bytes()
                .await?
                .ok_or("Temp file missing after write")?;
            final_file.write(temp_bytes).await?;
            // Best-effort delete - some backends (like tar) don't support deletion
            let _ = temp_file.delete().await;
        }

        let file = final_dir.file(&file_name)?;
        Ok(WriteResult { file, hash })
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::random_memory_dir;

    #[test]
    fn test_relative_path_simple_file() {
        let dir = random_memory_dir();
        let file = dir.file("test.parquet").unwrap();

        let relative = dir.relative_path(file.as_ref()).unwrap();
        assert_eq!(relative, "test.parquet");
    }

    #[test]
    fn test_relative_path_nested_file() {
        let dir = random_memory_dir();
        let subdir = dir.subdir("ab").unwrap();
        let file = subdir.file("cdef12345678.parquet").unwrap();

        let relative = dir.relative_path(file.as_ref()).unwrap();
        assert_eq!(relative, "ab/cdef12345678.parquet");
    }

    #[test]
    fn test_relative_path_deeply_nested() {
        let dir = random_memory_dir();
        let sub1 = dir.subdir("level1").unwrap();
        let sub2 = sub1.subdir("level2").unwrap();
        let file = sub2.file("deep.json").unwrap();

        let relative = dir.relative_path(file.as_ref()).unwrap();
        assert_eq!(relative, "level1/level2/deep.json");
    }

    #[test]
    fn test_relative_path_file_not_in_directory() {
        let dir1 = random_memory_dir();
        let dir2 = random_memory_dir();
        let file = dir2.file("other.csv").unwrap();

        let result = dir1.relative_path(file.as_ref());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("is not within directory"));
    }

    #[test]
    fn test_relative_path_sibling_directory() {
        let parent = random_memory_dir();
        let dir1 = parent.subdir("dir1").unwrap();
        let dir2 = parent.subdir("dir2").unwrap();
        let file = dir2.file("sibling.txt").unwrap();

        // File in dir2 should not be relative to dir1
        let result = dir1.relative_path(file.as_ref());
        assert!(result.is_err());
    }

    #[test]
    fn test_relative_path_from_subdir() {
        let dir = random_memory_dir();
        let subdir = dir.subdir("sub").unwrap();
        let file = subdir.file("nested.dat").unwrap();

        // From the subdir's perspective, the file is directly inside
        let relative = subdir.relative_path(file.as_ref()).unwrap();
        assert_eq!(relative, "nested.dat");
    }
}
