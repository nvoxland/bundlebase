//! IOFile implementation for file operations via object_store.

use crate::io::io_traits::{FileInfo, IOReader, IOWriter};
use crate::io::util::{compute_store_url, parse_url};
use crate::io::EMPTY_SCHEME;
use crate::BundleConfig;
use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use datafusion::execution::object_store::ObjectStoreUrl;
use futures::stream::{BoxStream, StreamExt, TryStreamExt};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectMeta, ObjectStore};
use serde::de::DeserializeOwned;
use serde::ser::Serialize;
use sha2::{Digest, Sha256};
use std::fmt::{Debug, Display};
use std::sync::Arc;
use url::Url;

/// File abstraction for reading and writing files via object_store.
/// Replaces IOFile with unified IO traits.
#[derive(Clone)]
pub struct IOFile {
    url: Url,
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
}

impl Debug for IOFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IOFile")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl IOFile {
    /// Create an IOFile from a URL.
    pub fn from_url(url: &Url, config: Arc<BundleConfig>) -> Result<IOFile, BundlebaseError> {
        let config_map = config.get_config_for_url(url);
        let (store, path) = parse_url(url, &config_map)?;
        Self::new(url, store, &path)
    }

    /// Creates a file from the passed string.
    /// The string can be either a URL or a path relative to the passed base_dir.
    pub fn from_str(
        path: &str,
        base: &crate::io::IODir,
        config: Arc<BundleConfig>,
    ) -> Result<IOFile, BundlebaseError> {
        if path.contains(":") {
            // Absolute URL - use provided config
            Self::from_url(&Url::parse(path)?, config)
        } else {
            // Relative path - get file from base directory
            // io_file() returns IOFile directly
            Ok(base.io_file(path)?)
        }
    }

    /// Create an IOFile directly with all components.
    pub fn new(
        url: &Url,
        store: Arc<dyn ObjectStore>,
        path: &ObjectPath,
    ) -> Result<Self, BundlebaseError> {
        Ok(Self {
            url: url.clone(),
            store,
            path: path.clone(),
        })
    }

    /// Get the filename portion of the path.
    pub fn filename(&self) -> &str {
        self.path.filename().unwrap_or("")
    }

    /// Get the underlying ObjectStore.
    pub fn store(&self) -> Arc<dyn ObjectStore> {
        self.store.clone()
    }

    /// Get the ObjectStore URL for DataFusion registration.
    pub fn store_url(&self) -> ObjectStoreUrl {
        compute_store_url(&self.url)
    }

    /// Get the path within the object store.
    pub fn store_path(&self) -> &ObjectPath {
        &self.path
    }

    /// Read file contents as a stream, returning an error if the file doesn't exist.
    pub async fn read_existing(
        &self,
    ) -> Result<BoxStream<'static, Result<Bytes, BundlebaseError>>, BundlebaseError> {
        match self.read_stream().await? {
            Some(stream) => Ok(stream),
            None => Err(format!("File not found: {}", self.url).into()),
        }
    }

    /// Read file contents and deserialize from YAML.
    pub async fn read_yaml<T>(&self) -> Result<Option<T>, BundlebaseError>
    where
        T: DeserializeOwned,
    {
        match self.read_str().await? {
            Some(str) => Ok(Some(serde_yaml::from_str(&str)?)),
            None => Ok(None),
        }
    }

    /// Serialize value to YAML and write to file.
    pub async fn write_yaml<T>(&self, value: &T) -> Result<(), BundlebaseError>
    where
        T: ?Sized + Serialize,
    {
        let yaml = serde_yaml::to_string(value)?;
        self.write(Bytes::from(yaml)).await
    }

    /// Get full ObjectMeta from object store.
    pub async fn object_meta(&self) -> Result<Option<ObjectMeta>, BundlebaseError> {
        match self.store.head(&self.path).await {
            Ok(meta) => Ok(Some(meta)),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(None)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }
}

#[async_trait]
impl IOReader for IOFile {
    fn url(&self) -> &Url {
        &self.url
    }

    async fn exists(&self) -> Result<bool, BundlebaseError> {
        match self.store.head(&self.path).await {
            Ok(_) => Ok(true),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(false)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }

    async fn read_bytes(&self) -> Result<Option<Bytes>, BundlebaseError> {
        match self.store.get(&self.path).await {
            Ok(r) => Ok(Some(r.bytes().await?)),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(None)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }

    async fn read_stream(
        &self,
    ) -> Result<Option<BoxStream<'static, Result<Bytes, BundlebaseError>>>, BundlebaseError> {
        match self.store.get(&self.path).await {
            Ok(result) => {
                let stream = result
                    .into_stream()
                    .map_err(|e| Box::new(e) as BundlebaseError);
                Ok(Some(Box::pin(stream)))
            }
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(None)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }

    async fn metadata(&self) -> Result<Option<FileInfo>, BundlebaseError> {
        match self.store.head(&self.path).await {
            Ok(meta) => Ok(Some(
                FileInfo::new(self.url.clone())
                    .with_size(meta.size as u64)
                    .with_modified(meta.last_modified),
            )),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(None)
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }

    async fn version(&self) -> Result<String, BundlebaseError> {
        let meta = self.store.head(&self.path).await?;
        // Priority: Version (S3 style) → ETag (HTTP standard) → LastModified (hashed timestamp)
        let version = if meta
            .version
            .as_ref()
            .is_some_and(|x| !x.is_empty() && x != "0")
        {
            meta.version
        } else if meta
            .e_tag
            .as_ref()
            .is_some_and(|x| !x.is_empty() && x != "0")
        {
            meta.e_tag
        } else {
            let timestamp = meta.last_modified.to_rfc3339();
            let mut hasher = Sha256::new();
            hasher.update(timestamp.as_bytes());
            let hash = hasher.finalize();
            Some(hex::encode(&hash[..8]))
        };
        Ok(version.unwrap_or_else(|| "UNKNOWN".to_string()))
    }
}

#[async_trait]
impl IOWriter for IOFile {
    async fn write(&self, data: Bytes) -> Result<(), BundlebaseError> {
        if self.url.scheme() == EMPTY_SCHEME {
            return Err(format!("Cannot write to {}:// URL: {}", EMPTY_SCHEME, self.url).into());
        }

        let put_result = object_store::PutPayload::from_bytes(data);
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    async fn write_stream_boxed(
        &self,
        mut source: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Result<(), BundlebaseError> {
        if self.url.scheme() == EMPTY_SCHEME {
            return Err(format!("Cannot write to {}:// URL: {}", EMPTY_SCHEME, self.url).into());
        }

        // Collect stream into a single buffer
        let mut buffer = Vec::new();
        while let Some(chunk_result) = source.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }

        let put_result = object_store::PutPayload::from_bytes(Bytes::from(buffer));
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    async fn delete(&self) -> Result<(), BundlebaseError> {
        match self.store.delete(&self.path).await {
            Ok(_) => Ok(()),
            Err(e) => {
                if matches!(e, object_store::Error::NotFound { .. }) {
                    Ok(())
                } else {
                    Err(Box::new(e))
                }
            }
        }
    }
}

impl Display for IOFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IOFile({})", self.url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::random_memory_file;

    #[test]
    fn test_filename() {
        let file = IOFile::from_url(
            &Url::parse("memory:///test/test.json").unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        assert_eq!(file.filename(), "test.json");
        assert_eq!(file.url().to_string(), "memory:///test/test.json");
    }

    #[tokio::test]
    async fn test_read_write() {
        let file = random_memory_file("test.json");
        // Convert to IOFile
        let io_file = IOFile::from_url(file.url(), BundleConfig::default().into()).unwrap();

        assert!(!io_file.exists().await.unwrap());

        io_file.write(Bytes::from("hello world")).await.unwrap();
        assert_eq!(
            Some(Bytes::from("hello world")),
            io_file.read_bytes().await.unwrap()
        );
    }

    #[tokio::test]
    async fn test_null() {
        let file = IOFile::from_url(
            &Url::parse("empty:///test.json").unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        assert!(!file.exists().await.unwrap());
        assert!(file.write(Bytes::from("hello world")).await.is_err());
    }
}
