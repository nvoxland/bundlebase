use crate::BundleConfig;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectMeta, ObjectStore};

use crate::io::util::{compute_store_url, parse_url};
use crate::io::{ObjectStoreDir, EMPTY_SCHEME};
use crate::BundlebaseError;
use datafusion::execution::object_store::ObjectStoreUrl;
use futures::stream::{StreamExt, TryStreamExt};
use serde::ser;
use sha2::{Digest, Sha256};
use std::fmt::Display;
use std::sync::Arc;
use url::Url;

#[derive(Debug, Clone)]
pub struct ObjectStoreFile {
    url: Url,
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
}

impl ObjectStoreFile {
    pub fn from_url(
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<ObjectStoreFile, BundlebaseError> {
        let config_map = config.get_config_for_url(url);
        let (store, path) = parse_url(url, &config_map)?;

        Self::new(&url, store, &path)
    }

    /// Creates a file from the passed string. The string can be either a URL or a path relative to the passed base_dir.
    pub fn from_str(
        path: &str,
        base: &ObjectStoreDir,
        config: Arc<BundleConfig>,
    ) -> Result<ObjectStoreFile, BundlebaseError> {
        if path.contains(":") {
            // Absolute URL - use provided config
            Self::from_url(&Url::parse(path)?, config)
        } else {
            // Relative path - config flows through base.file()
            base.file(path)
        }
    }

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

    pub fn filename(&self) -> &str {
        self.path.filename().unwrap()
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn store(&self) -> Arc<dyn ObjectStore> {
        self.store.clone()
    }

    pub fn store_url(&self) -> ObjectStoreUrl {
        compute_store_url(self.url())
    }

    pub fn store_path(&self) -> &ObjectPath {
        &self.path
    }

    pub async fn exists(&self) -> Result<bool, BundlebaseError> {
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

    /// Reads the file content, returning `None` if the file doesn't exist.
    /// Unless you know the file will be small, prefer using read_stream().
    /// Returns Err for any non "file-doesn't-exist" errors
    pub async fn read_bytes(&self) -> Result<Option<bytes::Bytes>, BundlebaseError> {
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

    pub async fn read_yaml<T>(&self) -> Result<Option<T>, BundlebaseError>
    where
        T: serde::de::DeserializeOwned,
    {
        let str = self.read_str().await?;

        match str {
            Some(str) => Ok(Some(serde_yaml::from_str(str.as_str())?)),
            None => Ok(None),
        }
    }

    /// Reads the file content as a UTF-8 string, returning `None` if the file doesn't exist.
    /// Unless you know the file will be small, prefer using read_stream().
    /// Returns Err for any non "file-doesn't-exist" errors
    pub async fn read_str(&self) -> Result<Option<String>, BundlebaseError> {
        match self.read_bytes().await? {
            Some(bytes) => Ok(Some(String::from_utf8(bytes.to_vec())?)),
            None => Ok(None),
        }
    }

    /// Reads the file content as a stream of bytes, returning `None` if the file doesn't exist.
    /// Still returns Err for any non "file-doesn't-exist" errors.
    pub async fn read(
        &self,
    ) -> Result<
        Option<futures::stream::BoxStream<'static, Result<bytes::Bytes, BundlebaseError>>>,
        BundlebaseError,
    > {
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

    /// Reads the file content as a stream of bytes, returning an error if the file doesn't exist.
    pub async fn read_existing(
        &self,
    ) -> Result<
        futures::stream::BoxStream<'static, Result<bytes::Bytes, BundlebaseError>>,
        BundlebaseError,
    > {
        match self.read().await? {
            Some(stream) => Ok(stream),
            None => Err(format!("File not found: {}", self.url).into()),
        }
    }

    /// Writes data to the file, overwriting if it exists.
    pub async fn write(&self, data: bytes::Bytes) -> Result<(), BundlebaseError> {
        if self.url.scheme() == EMPTY_SCHEME {
            return Err(format!("Cannot write to {}:// URL: {}", EMPTY_SCHEME, self.url).into());
        }

        let put_result = object_store::PutPayload::from_bytes(data);
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    /// Writes a stream of bytes to the file, overwriting if it exists.
    /// The stream is collected into a buffer before writing.
    pub async fn write_stream<S>(&self, mut source: S) -> Result<(), BundlebaseError>
    where
        S: futures::stream::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin,
    {
        if self.url.scheme() == EMPTY_SCHEME {
            return Err(format!("Cannot write to {}:// URL: {}", EMPTY_SCHEME, self.url).into());
        }

        // Collect stream into a single buffer
        let mut buffer = Vec::new();
        while let Some(chunk_result) = source.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }

        let put_result = object_store::PutPayload::from_bytes(bytes::Bytes::from(buffer));
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    pub async fn write_yaml<T>(&self, value: &T) -> Result<(), BundlebaseError>
    where
        T: ?Sized + ser::Serialize,
    {
        let yaml = serde_yaml::to_string(value)?;

        let data = bytes::Bytes::from(yaml);
        self.write(data).await?;
        Ok(())
    }

    /// Returns file metadata, or `None` if the file doesn't exist.
    pub async fn metadata(&self) -> Result<Option<ObjectMeta>, BundlebaseError> {
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

    /// Returns a version identifier for the file (e.g., ETag, last modified time, or version ID).
    pub async fn version(&self) -> Result<String, BundlebaseError> {
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
            Some(hex::encode(&hash[..8])) // Use first 8 bytes
        };
        Ok(version.unwrap_or_else(|| "UNKNOWN".to_string()))
    }

    /// Deletes the file, returning Ok even if the file doesn't exist.
    pub async fn delete(&self) -> Result<(), BundlebaseError> {
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

impl Display for ObjectStoreFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ObjectStoreFile({} {})", self.store, self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::random_memory_file;
    use crate::BundleConfig;

    #[test]
    fn test_filename() {
        let file = ObjectStoreFile::from_url(
            &Url::parse("memory:///test/test.json").unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        assert_eq!(file.filename(), "test.json");
        assert_eq!(file.url().to_string(), "memory:///test/test.json");

        let file = ObjectStoreFile::from_url(
            &Url::parse("memory:///test/dir/here/test.json").unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        assert_eq!("test/dir/here/test.json", file.path.to_string());
        assert_eq!(file.filename(), "test.json");
    }

    #[tokio::test]
    async fn test_read_write() {
        let file = random_memory_file("test.json");

        assert!(!file.exists().await.unwrap());
        assert_eq!(true, file.read_existing().await.is_err());
        assert_eq!(true, file.version().await.is_err());

        file.write(bytes::Bytes::from("hello world")).await.unwrap();
        assert_eq!(
            Some(bytes::Bytes::from("hello world")),
            file.read_bytes().await.unwrap()
        );
    }

    #[tokio::test]
    async fn test_null() {
        let file = ObjectStoreFile::from_url(
            &Url::parse("empty:///test.json").unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        assert!(!file.exists().await.unwrap());

        assert!(file.write(bytes::Bytes::from("hello world")).await.is_err());
    }

    #[tokio::test]
    async fn test_read_write_stream() {
        let file = random_memory_file("test_stream.json");

        assert_eq!(None, file.read().await.unwrap().map(|_| ()));
        assert!(file.read_existing().await.is_err());

        // Write using stream - create a simple stream from a vec of chunks
        let data = bytes::Bytes::from("hello stream world");
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(data)]);
        file.write_stream(stream).await.unwrap();

        // Read back using stream
        let mut stream = file.read_existing().await.unwrap();
        let mut buffer = Vec::new();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.unwrap();
            buffer.extend_from_slice(&chunk);
        }
        assert_eq!(
            bytes::Bytes::from("hello stream world"),
            bytes::Bytes::from(buffer)
        );
    }
}
