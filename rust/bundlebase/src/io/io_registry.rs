//! IO Registry for dispatching storage operations by URL scheme.
//!
//! Provides a central registry of storage backends that can be looked up by URL scheme.

use crate::io::io_dir::IODir;
use crate::io::io_file::IOFile;
use crate::io::io_traits::{IOLister, IOReader, IOWriter};
use crate::BundleConfig;
use crate::BundlebaseError;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use url::Url;

/// Factory for creating IO instances from URLs.
/// Each backend implements this trait to handle its supported URL schemes.
#[async_trait]
pub trait IOFactory: Send + Sync {
    /// URL schemes this factory handles (e.g., ["ftp"], ["scp", "sftp"], ["tar"]).
    fn schemes(&self) -> &[&str];

    /// Whether this backend supports write operations.
    fn supports_write(&self) -> bool;

    /// Create a reader for a file URL.
    async fn create_reader(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReader>, BundlebaseError>;

    /// Create a lister for a directory URL.
    async fn create_lister(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOLister>, BundlebaseError>;

    /// Create a writer for a file URL.
    /// Returns None if this backend is read-only.
    async fn create_writer(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Option<Box<dyn IOWriter>>, BundlebaseError>;
}

/// Factory for object_store-backed URLs (file://, s3://, gs://, azure://, memory://, empty://).
pub struct ObjectStoreIOFactory;

#[async_trait]
impl IOFactory for ObjectStoreIOFactory {
    fn schemes(&self) -> &[&str] {
        &["file", "s3", "gs", "azure", "az", "memory", "empty"]
    }

    fn supports_write(&self) -> bool {
        true
    }

    async fn create_reader(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReader>, BundlebaseError> {
        Ok(Box::new(IOFile::from_url(url, config)?))
    }

    async fn create_lister(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOLister>, BundlebaseError> {
        Ok(Box::new(IODir::from_url(url, config)?))
    }

    async fn create_writer(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Option<Box<dyn IOWriter>>, BundlebaseError> {
        // empty:// is read-only
        if url.scheme() == "empty" {
            return Ok(None);
        }
        Ok(Some(Box::new(IOFile::from_url(url, config)?)))
    }
}

/// Central registry for IO backends, dispatching by URL scheme.
pub struct IORegistry {
    factories: HashMap<String, Arc<dyn IOFactory>>,
}

impl IORegistry {
    /// Create a new registry with built-in factories.
    pub fn new() -> Self {
        let mut registry = Self {
            factories: HashMap::new(),
        };

        // Register built-in factories
        registry.register(Arc::new(ObjectStoreIOFactory));

        registry
    }

    /// Register a factory for its supported schemes.
    pub fn register(&mut self, factory: Arc<dyn IOFactory>) {
        for scheme in factory.schemes() {
            self.factories.insert(scheme.to_string(), factory.clone());
        }
    }

    /// Get the factory for a URL scheme.
    pub fn get_factory(&self, scheme: &str) -> Option<Arc<dyn IOFactory>> {
        self.factories.get(scheme).cloned()
    }

    /// Check if a scheme supports write operations.
    pub fn supports_write(&self, scheme: &str) -> bool {
        self.factories
            .get(scheme)
            .map(|f| f.supports_write())
            .unwrap_or(false)
    }

    /// Create a reader for any supported URL.
    pub async fn create_reader(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReader>, BundlebaseError> {
        let factory = self.get_factory(url.scheme()).ok_or_else(|| {
            format!("Unsupported URL scheme: {}", url.scheme())
        })?;
        factory.create_reader(url, config).await
    }

    /// Create a lister for any supported URL.
    pub async fn create_lister(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOLister>, BundlebaseError> {
        let factory = self.get_factory(url.scheme()).ok_or_else(|| {
            format!("Unsupported URL scheme: {}", url.scheme())
        })?;
        factory.create_lister(url, config).await
    }

    /// Create a writer for any supported URL.
    /// Returns an error if the scheme is read-only.
    pub async fn create_writer(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOWriter>, BundlebaseError> {
        let factory = self.get_factory(url.scheme()).ok_or_else(|| {
            format!("Unsupported URL scheme: {}", url.scheme())
        })?;

        factory
            .create_writer(url, config)
            .await?
            .ok_or_else(|| format!("Storage scheme '{}' is read-only", url.scheme()).into())
    }
}

impl Default for IORegistry {
    fn default() -> Self {
        Self::new()
    }
}

// Global singleton registry
static IO_REGISTRY: OnceLock<IORegistry> = OnceLock::new();

/// Get the global IO registry instance.
pub fn io_registry() -> &'static IORegistry {
    IO_REGISTRY.get_or_init(IORegistry::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_has_builtin_factories() {
        let registry = IORegistry::new();

        assert!(registry.get_factory("file").is_some());
        assert!(registry.get_factory("s3").is_some());
        assert!(registry.get_factory("gs").is_some());
        assert!(registry.get_factory("azure").is_some());
        assert!(registry.get_factory("memory").is_some());
        assert!(registry.get_factory("empty").is_some());
    }

    #[test]
    fn test_supports_write() {
        let registry = IORegistry::new();

        assert!(registry.supports_write("file"));
        assert!(registry.supports_write("s3"));
        assert!(registry.supports_write("memory"));
        // empty:// is handled by ObjectStoreIOFactory but returns None for writer
        assert!(registry.supports_write("empty")); // Factory says yes, but create_writer returns None

        // Unknown scheme
        assert!(!registry.supports_write("unknown"));
    }

    #[tokio::test]
    async fn test_create_reader() {
        let registry = IORegistry::new();
        let url = Url::parse("memory:///test/file.txt").unwrap();
        let config = BundleConfig::default().into();

        let reader = registry.create_reader(&url, config).await;
        assert!(reader.is_ok());
    }

    #[tokio::test]
    async fn test_create_reader_unknown_scheme() {
        let registry = IORegistry::new();
        let url = Url::parse("unknown:///test").unwrap();
        let config = BundleConfig::default().into();

        let result = registry.create_reader(&url, config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported URL scheme"));
    }
}
