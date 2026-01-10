//! IODir implementation for directory operations via object_store.

use crate::io::io_file::IOFile;
use crate::io::io_traits::{FileInfo, IOLister, IOReader};
use crate::io::util::{join_path, join_url, parse_url};
use crate::io::{EMPTY_SCHEME, EMPTY_URL};
use crate::BundleConfig;
use crate::BundlebaseError;
use async_trait::async_trait;
use futures::stream::StreamExt;
use object_store::path::Path as ObjectPath;
use object_store::ObjectStore;
use std::collections::HashMap;
use std::env::current_dir;
use std::fmt::{Debug, Display};
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;

/// Directory abstraction for listing files and navigating subdirectories.
/// Replaces IODir with unified IO traits.
#[derive(Clone)]
pub struct IODir {
    url: Url,
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    config: Arc<BundleConfig>,
}

impl Debug for IODir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IODir")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl IODir {
    /// Create an IODir from a URL.
    pub fn from_url(url: &Url, config: Arc<BundleConfig>) -> Result<IODir, BundlebaseError> {
        if url.scheme() == "memory" && !url.authority().is_empty() {
            return Err("Memory URL must be memory:///<path>".into());
        }
        if url.scheme() == EMPTY_SCHEME && !url.authority().is_empty() {
            return Err(format!("Empty URL must be {}<path>", EMPTY_URL).into());
        }

        let config_map = config.get_config_for_url(url);
        let (store, path) = parse_url(url, &config_map)?;

        IODir::new(url, store, &path, config)
    }

    /// Creates a directory from the passed string.
    /// The string can be either a URL or a filesystem path (relative or absolute).
    pub fn from_str(path: &str, config: Arc<BundleConfig>) -> Result<IODir, BundlebaseError> {
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

    /// Creates a memory-backed directory for storing index and metadata files.
    pub fn new_memory() -> Result<IODir, BundlebaseError> {
        let url = Url::parse("memory:///_indexes")?;
        let config = HashMap::new();
        let (store, path) = parse_url(&url, &config)?;
        IODir::new(&url, store, &path, BundleConfig::default().into())
    }

    /// Get the underlying ObjectStore.
    pub fn store(&self) -> Arc<dyn ObjectStore> {
        self.store.clone()
    }

    /// Get the configuration.
    pub fn config(&self) -> Arc<BundleConfig> {
        self.config.clone()
    }

    /// Get an IOFile for a path within this directory.
    pub fn io_file(&self, path: &str) -> Result<IOFile, BundlebaseError> {
        let file_url = join_url(&self.url, path)?;
        let object_path = join_path(&self.path, path)?;

        // Reuse the existing store instead of creating a new one
        // This is important for stores like TarObjectStore where the URL might not
        // indicate the store type
        IOFile::new(&file_url, self.store.clone(), &object_path)
    }

    /// Get an IODir for a subdirectory within this directory.
    pub fn io_subdir(&self, subdir: &str) -> Result<IODir, BundlebaseError> {
        Ok(IODir {
            url: join_url(&self.url, subdir)?,
            store: self.store.clone(),
            path: join_path(&self.path, subdir)?,
            config: self.config.clone(),
        })
    }
}

#[async_trait]
impl IOLister for IODir {
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
                    .with_size(meta.size as u64)
                    .with_modified(meta.last_modified),
            );
        }
        Ok(files)
    }

    fn subdir(&self, name: &str) -> Result<Box<dyn IOLister>, BundlebaseError> {
        Ok(Box::new(self.io_subdir(name)?))
    }

    fn file(&self, name: &str) -> Result<Box<dyn IOReader>, BundlebaseError> {
        Ok(Box::new(self.io_file(name)?))
    }
}

impl Display for IODir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url)
    }
}

fn str_to_url(path: &str) -> Result<Url, BundlebaseError> {
    if path.contains(":") {
        Ok(Url::parse(path)?)
    } else {
        // Check if this is a tar file - if so, use tar:// scheme
        let file = file_url(path);
        if file.path().ends_with(".tar") {
            Ok(Url::parse(&format!("tar://{}", file.path()))?)
        } else {
            Ok(file)
        }
    }
}

/// Returns a URL for a file path.
/// If the path is relative, returns an absolute file URL relative to the current working directory.
fn file_url(path: &str) -> Url {
    let path_buf = PathBuf::from(path);
    let absolute_path = if path_buf.is_absolute() {
        path_buf
    } else {
        current_dir().unwrap().join(path_buf)
    };

    Url::from_file_path(absolute_path.as_path()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

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
        let dir = IODir::from_str(input, BundleConfig::default().into()).unwrap();
        assert_eq!(dir.url.to_string(), input);
        assert_eq!(dir.path.to_string(), expected_path);
    }

    #[test]
    fn test_from_string_complex() {
        assert!(
            IODir::from_str("memory://bucket/test", BundleConfig::default().into()).is_err(),
            "Memory must start with :///"
        );

        let dir =
            IODir::from_str("memory:///test/../test2", BundleConfig::default().into()).unwrap();
        assert_eq!(dir.path.to_string(), "test2");
        assert_eq!(dir.url.to_string(), "memory:///test2");

        let dir = IODir::from_str("relative/path", BundleConfig::default().into()).unwrap();
        assert_eq!(dir.url.to_string(), file_url("relative/path").to_string());
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
        let dir = IODir::from_url(&base, BundleConfig::default().into()).unwrap();
        let subdir = dir.io_subdir(subdir).unwrap();
        assert_eq!(subdir.url, expected_url);
        assert_eq!(subdir.path.to_string(), expected_path);
    }

    #[test]
    fn test_file() {
        let dir = IODir::from_str("memory:///test", BundleConfig::default().into()).unwrap();
        let file = dir.io_file("other").unwrap();
        assert_eq!(file.url().to_string(), "memory:///test/other");

        let subdir = dir.io_subdir("this/file.txt").unwrap();
        assert_eq!(subdir.url().to_string(), "memory:///test/this/file.txt");
    }

    #[tokio::test]
    async fn test_list_files() {
        let dir = IODir::from_str("memory:///test", BundleConfig::default().into()).unwrap();
        assert_eq!(0, dir.list_files().await.unwrap().len())
    }

    #[tokio::test]
    async fn test_null_url() {
        let dir = IODir::from_str(EMPTY_URL, BundleConfig::default().into()).unwrap();
        assert_eq!(0, dir.list_files().await.unwrap().len());
    }
}
