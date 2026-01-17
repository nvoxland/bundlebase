//! VersionedObjectStoreFile - wrapper that validates file version on first access.
//!
//! This wrapper ensures that files haven't changed since they were attached to a bundle.
//! It validates the version on first data access and caches the result for subsequent calls.

use super::object_store::ObjectStoreFile;
use crate::io::{FileInfo, IOReadFile};
use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use datafusion::datasource::object_store::ObjectStoreUrl;
use futures::stream::BoxStream;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectMeta, ObjectStore};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use url::Url;

/// Wrapper around ObjectStoreFile that validates version on first access.
/// Once validated, subsequent calls are passed through without re-checking.
#[derive(Debug)]
pub struct VersionedObjectStoreFile {
    inner: ObjectStoreFile,
    expected_version: String,
    validated: AtomicBool,
}

impl VersionedObjectStoreFile {
    pub fn new(inner: ObjectStoreFile, expected_version: String) -> Self {
        Self {
            inner,
            expected_version,
            validated: AtomicBool::new(false),
        }
    }

    async fn validate_if_needed(&self) -> Result<(), BundlebaseError> {
        if self.validated.load(Ordering::Acquire) {
            return Ok(());
        }

        let current = self.inner.version().await?;
        if current != self.expected_version {
            return Err(BundlebaseError::from(format!(
                "Version mismatch for '{}': expected '{}', found '{}'. \
                 The source file has changed since the bundle was created.",
                self.inner.url(),
                self.expected_version,
                current
            )));
        }

        self.validated.store(true, Ordering::Release);
        Ok(())
    }

    /// Get the underlying ObjectStore.
    pub fn store(&self) -> Arc<dyn ObjectStore> {
        self.inner.store()
    }

    /// Get the ObjectStore URL for DataFusion registration.
    pub fn store_url(&self) -> ObjectStoreUrl {
        self.inner.store_url()
    }

    /// Get the path within the object store.
    pub fn store_path(&self) -> &ObjectPath {
        self.inner.store_path()
    }

    /// Get full ObjectMeta from object store, validating version first.
    pub async fn object_meta(&self) -> Result<Option<ObjectMeta>, BundlebaseError> {
        self.validate_if_needed().await?;
        self.inner.object_meta().await
    }

    /// Get access to the inner ObjectStoreFile.
    ///
    /// Use this when you need direct ObjectStoreFile access (e.g., for RowIdOffsetDataSource).
    /// Note: This bypasses version validation, so ensure you've validated first if needed.
    pub fn inner(&self) -> &ObjectStoreFile {
        &self.inner
    }
}

impl Clone for VersionedObjectStoreFile {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            expected_version: self.expected_version.clone(),
            validated: AtomicBool::new(self.validated.load(Ordering::Acquire)),
        }
    }
}

#[async_trait]
impl IOReadFile for VersionedObjectStoreFile {
    fn url(&self) -> &Url {
        self.inner.url()
    }

    async fn exists(&self) -> Result<bool, BundlebaseError> {
        self.inner.exists().await
    }

    async fn read_bytes(&self) -> Result<Option<Bytes>, BundlebaseError> {
        self.validate_if_needed().await?;
        self.inner.read_bytes().await
    }

    async fn read_stream(
        &self,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, BundlebaseError>>>, BundlebaseError> {
        self.validate_if_needed().await?;
        self.inner.read_stream().await
    }

    async fn metadata(&self) -> Result<Option<FileInfo>, BundlebaseError> {
        self.validate_if_needed().await?;
        self.inner.metadata().await
    }

    async fn version(&self) -> Result<String, BundlebaseError> {
        self.inner.version().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::IOReadWriteFile;
    use crate::test_utils::random_memory_dir_concrete;

    #[tokio::test]
    async fn test_versioned_file_validates_on_first_access() {
        // Create file, get version
        let dir = random_memory_dir_concrete();
        let file = dir.io_file("test.txt").unwrap();
        file.write(Bytes::from("hello world")).await.unwrap();
        let version = file.version().await.unwrap();

        // Create VersionedObjectStoreFile with correct version
        let versioned = VersionedObjectStoreFile::new(file.clone(), version);

        // Call object_meta() and verify it succeeds
        let meta = versioned.object_meta().await.unwrap();
        assert!(meta.is_some());

        // Verify validation was performed
        assert!(versioned.validated.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_versioned_file_version_mismatch() {
        // Create file, get version
        let dir = random_memory_dir_concrete();
        let file = dir.io_file("test.txt").unwrap();
        file.write(Bytes::from("hello world")).await.unwrap();
        let old_version = file.version().await.unwrap();

        // Modify file (changes version)
        file.write(Bytes::from("modified content")).await.unwrap();

        // Create VersionedObjectStoreFile with OLD version
        let versioned = VersionedObjectStoreFile::new(file.clone(), old_version);

        // Call object_meta() and verify version mismatch error
        let result = versioned.object_meta().await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Version mismatch"),
            "Expected version mismatch error, got: {}",
            err
        );
        assert!(
            err.contains("source file has changed"),
            "Expected explanation about file change, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_versioned_file_caches_validation() {
        // Create file
        let dir = random_memory_dir_concrete();
        let file = dir.io_file("test.txt").unwrap();
        file.write(Bytes::from("hello world")).await.unwrap();
        let version = file.version().await.unwrap();

        // Create VersionedObjectStoreFile
        let versioned = VersionedObjectStoreFile::new(file.clone(), version);

        // Verify not validated yet
        assert!(!versioned.validated.load(Ordering::Acquire));

        // Call object_meta() first time
        versioned.object_meta().await.unwrap();

        // Verify validated is now true
        assert!(versioned.validated.load(Ordering::Acquire));

        // Call object_meta() again - validation should be cached
        // (we can't easily verify version() wasn't called again, but we can verify
        // that the validated flag stays true and we don't get errors)
        versioned.object_meta().await.unwrap();
        assert!(versioned.validated.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_versioned_file_read_bytes_validates() {
        let dir = random_memory_dir_concrete();
        let file = dir.io_file("test.txt").unwrap();
        file.write(Bytes::from("hello")).await.unwrap();
        let version = file.version().await.unwrap();

        let versioned = VersionedObjectStoreFile::new(file, version);

        // read_bytes should trigger validation
        let bytes = versioned.read_bytes().await.unwrap();
        assert_eq!(bytes, Some(Bytes::from("hello")));
        assert!(versioned.validated.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_versioned_file_clone_preserves_validation_state() {
        let dir = random_memory_dir_concrete();
        let file = dir.io_file("test.txt").unwrap();
        file.write(Bytes::from("hello")).await.unwrap();
        let version = file.version().await.unwrap();

        let versioned = VersionedObjectStoreFile::new(file, version);

        // Clone before validation
        let clone1 = versioned.clone();
        assert!(!clone1.validated.load(Ordering::Acquire));

        // Validate the original
        versioned.object_meta().await.unwrap();
        assert!(versioned.validated.load(Ordering::Acquire));

        // Clone after validation should preserve validated state
        let clone2 = versioned.clone();
        assert!(clone2.validated.load(Ordering::Acquire));
    }
}
