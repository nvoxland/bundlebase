//! ObjectStore IO backend - file and directory operations via the object_store crate.
//!
//! Supports: file://, s3://, gs://, azure://, az://, memory://, empty://

use crate::io::registry::IOFactory;
use crate::io::{FileInfo, IOReadDir, IOReadFile, IOReadWriteDir, IOReadWriteFile};
use crate::io::util::{join_path, join_url};
use crate::io::{get_memory_store, get_null_store, EMPTY_SCHEME, EMPTY_URL};
use crate::BundleConfig;
use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use datafusion::datasource::object_store::ObjectStoreUrl;
use futures::stream::{BoxStream, StreamExt, TryStreamExt};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectMeta, ObjectStore};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env::current_dir;
use std::fmt::{Debug, Display};
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;

use super::tar::TarObjectStore;

// ============================================================================
// URL and ObjectStore utilities
// ============================================================================

pub(crate) fn compute_store_url(url: &Url) -> ObjectStoreUrl {
    ObjectStoreUrl::parse(format!("{}://{}", url.scheme(), url.authority())).expect("BUG: URL scheme://authority should be valid")
}

/// Parse a URL and return an ObjectStore and Path
///
/// # Arguments
/// * `url` - The URL to parse
/// * `config` - Optional configuration to apply to the ObjectStore
pub(crate) fn parse_url(
    url: &Url,
    config: &HashMap<String, String>,
) -> Result<(Arc<dyn ObjectStore>, ObjectPath), BundlebaseError> {
    // Handle tar:// scheme - format is tar:///path/to/archive.tar or tar:///path/to/archive.tar/internal/path
    if url.scheme() == "tar" {
        let full_path = url.path();
        // Find where the .tar file ends and internal path begins
        let (tar_path, internal_path) = if let Some(tar_idx) = full_path.find(".tar") {
            let tar_end = tar_idx + 4; // ".tar" is 4 chars
            let tar_file = &full_path[..tar_end];
            let internal = if tar_end < full_path.len() {
                &full_path[tar_end..]
            } else {
                "/"
            };
            (tar_file, internal)
        } else {
            // No .tar found, treat entire path as tar file
            (full_path, "/")
        };

        let store = TarObjectStore::new(std::path::PathBuf::from(tar_path)).map_err(|e| {
            format!("Failed to create TarObjectStore for {}: {}", tar_path, e)
        })?;
        return Ok((Arc::new(store), ObjectPath::from(internal_path)));
    }

    // Check for .tar file extension first (before other file:// handling)
    if url.scheme() == "file" {
        if let Ok(path) = url.to_file_path() {
            if path.extension().and_then(|s| s.to_str()) == Some("tar") {
                let store = TarObjectStore::new(path).map_err(|e| {
                    format!("Failed to create TarObjectStore: {}", e)
                })?;
                return Ok((Arc::new(store), ObjectPath::from("/")));
            }
        }
    }

    if url.scheme() == EMPTY_SCHEME {
        let store: Arc<dyn ObjectStore> = get_null_store();

        if !url.authority().is_empty() {
            return Err("Empty URL must be empty:///<path>.".into());
        }
        Ok((store, ObjectPath::from(url.path())))
    } else if url.scheme() == "memory" {
        if !url.authority().is_empty() {
            return Err("Memory URL must be memory:///<path>".into());
        }
        Ok((get_memory_store(), url.path().into()))
    } else if !config.is_empty() {
        // Use config to build ObjectStore
        let store = build_object_store(url, config)?;
        let path = ObjectPath::from(url.path());
        Ok((Arc::new(store), path))
    } else {
        // Fallback to object_store::parse_url when no config
        let (store, path) = object_store::parse_url(url)?;
        Ok((Arc::new(store), path))
    }
}

/// Build an ObjectStore with configuration
///
/// Starts with Builder::from_env() to pick up environment variables,
/// then applies config values on top (config overrides env vars).
fn build_object_store(
    url: &Url,
    config: &HashMap<String, String>,
) -> Result<Box<dyn ObjectStore>, BundlebaseError> {
    use object_store::aws::AmazonS3Builder;
    use object_store::azure::MicrosoftAzureBuilder;
    use object_store::gcp::GoogleCloudStorageBuilder;

    match url.scheme() {
        "s3" => {
            let mut builder = AmazonS3Builder::from_env().with_url(url.as_str());

            // Apply config values
            for (key, value) in config {
                builder = builder.with_config(key.parse()?, value);
            }

            Ok(Box::new(builder.build()?))
        }
        "gs" => {
            let mut builder = GoogleCloudStorageBuilder::from_env().with_url(url.as_str());

            // Apply config values
            for (key, value) in config {
                builder = builder.with_config(key.parse()?, value);
            }

            Ok(Box::new(builder.build()?))
        }
        "azure" | "az" => {
            let mut builder = MicrosoftAzureBuilder::from_env().with_url(url.as_str());

            // Apply config values
            for (key, value) in config {
                builder = builder.with_config(key.parse()?, value);
            }

            Ok(Box::new(builder.build()?))
        }
        scheme => {
            // For unknown schemes, fall back to object_store::parse_url
            let (store, _) = object_store::parse_url(url)
                .map_err(|e| format!("Unsupported URL scheme '{}': {}", scheme, e))?;
            Ok(Box::new(store))
        }
    }
}

// ============================================================================
// IOFile - File abstraction for reading and writing files via object_store
// ============================================================================

/// File abstraction for reading and writing files via object_store.
#[derive(Clone)]
pub struct ObjectStoreFile {
    url: Url,
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
}

impl Debug for ObjectStoreFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IOFile")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl ObjectStoreFile {
    /// Create an IOFile from a URL.
    pub fn from_url(url: &Url, config: Arc<BundleConfig>) -> Result<ObjectStoreFile, BundlebaseError> {
        let config_map = config.get_config_for_url(url);
        let (store, path) = parse_url(url, &config_map)?;
        Self::new(url, store, &path)
    }

    /// Creates a file from the passed string.
    /// The string can be either a URL or a path relative to the passed base_dir.
    pub fn from_str(
        path: &str,
        base: &dyn IOReadDir,
        config: Arc<BundleConfig>,
    ) -> Result<ObjectStoreFile, BundlebaseError> {
        if path.contains(":") {
            // Absolute URL - use provided config
            Self::from_url(&Url::parse(path)?, config)
        } else {
            // Relative path - join with base URL and create from that
            let base_url = base.url();
            let file_url = join_url(base_url, path)?;
            Self::from_url(&file_url, config)
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
impl IOReadFile for ObjectStoreFile {
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
                    .with_size(meta.size)
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
impl IOReadWriteFile for ObjectStoreFile {
    async fn write(&self, data: Bytes) -> Result<(), BundlebaseError> {
        if self.url.scheme() == EMPTY_SCHEME {
            return Err(format!("Cannot write to {}:// URL: {}", EMPTY_SCHEME, self.url).into());
        }

        let put_result = object_store::PutPayload::from_bytes(data);
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    async fn write_stream(
        &self,
        mut source: BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Result<(), BundlebaseError> {
        if self.url.scheme() == EMPTY_SCHEME {
            return Err(format!("Cannot write to {}:// URL: {}", EMPTY_SCHEME, self.url).into());
        }

        // TODO: actually stream it
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

impl Display for ObjectStoreFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IOFile({})", self.url)
    }
}

// ============================================================================
// IODir - Directory abstraction for listing files and navigating subdirectories
// ============================================================================

/// Directory abstraction for listing files and navigating subdirectories.
#[derive(Clone)]
pub struct ObjectStoreDir {
    url: Url,
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    config: Arc<BundleConfig>,
}

impl Debug for ObjectStoreDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IODir")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl ObjectStoreDir {
    /// Create an IODir from a URL.
    pub fn from_url(url: &Url, config: Arc<BundleConfig>) -> Result<ObjectStoreDir, BundlebaseError> {
        if url.scheme() == "memory" && !url.authority().is_empty() {
            return Err("Memory URL must be memory:///<path>".into());
        }
        if url.scheme() == EMPTY_SCHEME && !url.authority().is_empty() {
            return Err(format!("Empty URL must be {}<path>", EMPTY_URL).into());
        }

        let config_map = config.get_config_for_url(url);
        let (store, path) = parse_url(url, &config_map)?;

        ObjectStoreDir::new(url, store, &path, config)
    }

    /// Creates a directory from the passed string.
    /// The string can be either a URL or a filesystem path (relative or absolute).
    pub fn from_str(path: &str, config: Arc<BundleConfig>) -> Result<ObjectStoreDir, BundlebaseError> {
        let url = str_to_url(path)?;
        Self::from_url(&url, config)
    }

    /// Create an IODir directly with all components.
    pub fn new(
        url: &Url,
        store: Arc<dyn ObjectStore>,
        path: &ObjectPath,
        config: Arc<BundleConfig>,
    ) -> Result<Self, BundlebaseError> {
        Ok(Self {
            url: url.clone(),
            store,
            path: path.clone(),
            config,
        })
    }

    /// Get an IOFile for a path within this directory.
    pub fn io_file(&self, path: &str) -> Result<ObjectStoreFile, BundlebaseError> {
        let file_url = join_url(&self.url, path)?;
        let object_path = join_path(&self.path, path)?;

        // Reuse the existing store instead of creating a new one
        // This is important for stores like TarObjectStore where the URL might not
        // indicate the store type
        ObjectStoreFile::new(&file_url, self.store.clone(), &object_path)
    }

    /// Get an IODir for a subdirectory within this directory.
    pub fn io_subdir(&self, subdir: &str) -> Result<ObjectStoreDir, BundlebaseError> {
        Ok(ObjectStoreDir {
            url: join_url(&self.url, subdir)?,
            store: self.store.clone(),
            path: join_path(&self.path, subdir)?,
            config: self.config.clone(),
        })
    }

}

#[async_trait]
impl IOReadDir for ObjectStoreDir {
    fn url(&self) -> &Url {
        &self.url
    }

    async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError> {
        let mut files = Vec::new();
        let mut list_iter = self.store.list(Some(&self.path));

        while let Some(meta_result) = list_iter.next().await {
            let meta = meta_result?;
            let location = meta.location;
            // Get the relative path from self.path to location by stripping the prefix
            let location_str = location.as_ref();
            let prefix_str = self.path.as_ref();
            let relative_path = if let Some(stripped) = location_str.strip_prefix(prefix_str) {
                stripped.trim_start_matches('/')
            } else {
                location_str
            };

            let file_url = join_url(&self.url, relative_path)?;
            files.push(
                FileInfo::new(file_url)
                    .with_size(meta.size)
                    .with_modified(meta.last_modified),
            );
        }
        Ok(files)
    }

    fn subdir(&self, name: &str) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        Ok(Box::new(self.io_subdir(name)?))
    }

    fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        Ok(Box::new(self.io_file(name)?))
    }
}

#[async_trait]
impl IOReadWriteDir for ObjectStoreDir {
    fn writable_subdir(&self, name: &str) -> Result<Box<dyn IOReadWriteDir>, BundlebaseError> {
        Ok(Box::new(self.io_subdir(name)?))
    }

    fn writable_file(&self, name: &str) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError> {
        Ok(Box::new(self.io_file(name)?))
    }

    async fn rename(&self, from: &str, to: &str) -> Result<(), BundlebaseError> {
        let from_path = join_path(&self.path, from)?;
        let to_path = join_path(&self.path, to)?;

        // Use native rename - atomic on local filesystem, efficient on cloud
        self.store.rename(&from_path, &to_path).await?;
        Ok(())
    }
}

impl Display for ObjectStoreDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url)
    }
}

fn str_to_url(path: &str) -> Result<Url, BundlebaseError> {
    if path.contains(":") {
        Ok(Url::parse(path)?)
    } else {
        // Check if this is a tar file - if so, use tar:// scheme
        let file = file_url(path)?;
        if file.path().ends_with(".tar") {
            Ok(Url::parse(&format!("tar://{}", file.path()))?)
        } else {
            Ok(file)
        }
    }
}

/// Returns a URL for a file path.
/// If the path is relative, returns an absolute file URL relative to the current working directory.
fn file_url(path: &str) -> Result<Url, BundlebaseError> {
    let path_buf = PathBuf::from(path);
    let absolute_path = if path_buf.is_absolute() {
        path_buf
    } else {
        current_dir().map_err(|e| {
            BundlebaseError::from(format!("Failed to get current directory: {}", e))
        })?.join(path_buf)
    };

    Url::from_file_path(absolute_path.as_path()).map_err(|_| {
        BundlebaseError::from(format!("Invalid file path: {}", path))
    })
}

// ============================================================================
// ObjectStoreIOFactory - Factory for creating ObjectStore-backed IO instances
// ============================================================================

/// Factory for object_store-backed URLs (file://, s3://, gs://, azure://, memory://, empty://).
pub struct ObjectStoreIOFactory;

#[async_trait]
impl IOFactory for ObjectStoreIOFactory {
    fn schemes(&self) -> &[&str] {
        &["file", "s3", "gs", "azure", "az", "memory", "empty"]
    }

    fn supports_write(&self, url: &Url) -> bool {
        // empty:// is read-only
        url.scheme() != "empty"
    }

    async fn create_reader(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        Ok(Box::new(ObjectStoreFile::from_url(url, config)?))
    }

    async fn create_lister(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        Ok(Box::new(ObjectStoreDir::from_url(url, config)?))
    }

    async fn create_writable_lister(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Option<Box<dyn IOReadWriteDir>>, BundlebaseError> {
        // empty:// is read-only
        if url.scheme() == "empty" {
            return Ok(None);
        }
        Ok(Some(Box::new(ObjectStoreDir::from_url(url, config)?)))
    }

    async fn create_writer(
        &self,
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<Option<Box<dyn IOReadWriteFile>>, BundlebaseError> {
        // empty:// is read-only
        if url.scheme() == "empty" {
            return Ok(None);
        }
        Ok(Some(Box::new(ObjectStoreFile::from_url(url, config)?)))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::random_memory_file;
    use rstest::rstest;

    #[tokio::test]
    async fn test_read_write() {
        let file = random_memory_file("test.json");
        // Convert to IOFile
        let io_file = ObjectStoreFile::from_url(file.url(), BundleConfig::default().into()).unwrap();

        assert!(!io_file.exists().await.unwrap());

        io_file.write(Bytes::from("hello world")).await.unwrap();
        assert_eq!(
            Some(Bytes::from("hello world")),
            io_file.read_bytes().await.unwrap()
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
        assert!(file.write(Bytes::from("hello world")).await.is_err());
    }

    // IODir tests

    #[rstest]
    #[case("memory:///test", "test")]
    #[case("memory:///test/", "test")]
    #[case("memory:///test/sub/dir", "test/sub/dir")]
    #[case("memory:///path//with///more/", "path/with/more")]
    #[case("file:///test", "test")]
    #[case("file:///test/sub/dir", "test/sub/dir")]
    #[case("s3://test", "")]
    #[case("s3://test/path/here", "path/here")]
    fn test_from_str(#[case] input: &str, #[case] expected_path: &str) {
        let dir = ObjectStoreDir::from_str(input, BundleConfig::default().into()).unwrap();
        assert_eq!(dir.url.to_string(), input);
        assert_eq!(dir.path.to_string(), expected_path);
    }

    #[test]
    fn test_from_string_complex() {
        assert!(
            ObjectStoreDir::from_str("memory://bucket/test", BundleConfig::default().into()).is_err(),
            "Memory must start with :///"
        );

        let dir =
            ObjectStoreDir::from_str("memory:///test/../test2", BundleConfig::default().into()).unwrap();
        assert_eq!(dir.path.to_string(), "test2");
        assert_eq!(dir.url.to_string(), "memory:///test2");

        let dir = ObjectStoreDir::from_str("relative/path", BundleConfig::default().into()).unwrap();
        assert_eq!(dir.url.to_string(), file_url("relative/path").unwrap().to_string());
    }

    #[rstest]
    #[case("memory:///test", "subdir", "memory:///test/subdir", "test/subdir")]
    #[case("memory:///test", "/subdir", "memory:///test/subdir", "test/subdir")]
    #[case("memory:///test/", "subdir", "memory:///test/subdir", "test/subdir")]
    #[case("memory:///test/", "/subdir", "memory:///test/subdir", "test/subdir")]
    #[case(
        "memory:///test",
        "/nested/subdir/here",
        "memory:///test/nested/subdir/here",
        "test/nested/subdir/here"
    )]
    fn test_subdir(
        #[case] base: Url,
        #[case] subdir: &str,
        #[case] expected_url: Url,
        #[case] expected_path: &str,
    ) {
        let dir = ObjectStoreDir::from_url(&base, BundleConfig::default().into()).unwrap();
        let subdir = dir.io_subdir(subdir).unwrap();
        assert_eq!(subdir.url, expected_url);
        assert_eq!(subdir.path.to_string(), expected_path);
    }

    #[test]
    fn test_file() {
        let dir = ObjectStoreDir::from_str("memory:///test", BundleConfig::default().into()).unwrap();
        let file = dir.io_file("other").unwrap();
        assert_eq!(file.url().to_string(), "memory:///test/other");

        let subdir = dir.io_subdir("this/file.txt").unwrap();
        assert_eq!(subdir.url().to_string(), "memory:///test/this/file.txt");
    }

    #[tokio::test]
    async fn test_list_files() {
        let dir = ObjectStoreDir::from_str("memory:///test", BundleConfig::default().into()).unwrap();
        assert_eq!(0, dir.list_files().await.unwrap().len())
    }

    #[tokio::test]
    async fn test_null_url() {
        let dir = ObjectStoreDir::from_str(EMPTY_URL, BundleConfig::default().into()).unwrap();
        assert_eq!(0, dir.list_files().await.unwrap().len());
    }

    // Utility tests

    #[rstest]
    #[case("s3://bucket/path/to/dir", "s3://bucket/")]
    #[case("s3://bucket/path/to/dir", "s3://bucket/")]
    #[case("memory:///path/to/dir", "memory:///")]
    #[case("memory:///path/to/dir", "memory:///")]
    fn test_compute_store_url(#[case] url: &str, #[case] expected: &str) {
        let url = Url::parse(url).unwrap();
        assert_eq!(expected, compute_store_url(&url).as_str());
    }
}
