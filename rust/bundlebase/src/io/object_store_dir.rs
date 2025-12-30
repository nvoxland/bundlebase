use crate::BundleConfig;
use object_store::{path::Path as ObjectPath, ObjectStore};
use std::env;

use crate::io::util::{join_path, join_url, parse_url};
use crate::io::{ObjectStoreFile, EMPTY_SCHEME, EMPTY_URL};
use crate::BundlebaseError;
use env::current_dir;
use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;

#[derive(Debug, Clone)]
pub struct ObjectStoreDir {
    url: Url,
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    config: Arc<BundleConfig>,
}

impl ObjectStoreDir {
    pub fn from_url(
        url: &Url,
        config: Arc<BundleConfig>,
    ) -> Result<ObjectStoreDir, BundlebaseError> {
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

    /// Creates a directory from the passed string. The string can be either a URL or a filesystem path (relative or absolute).
    pub fn from_str(
        path: &str,
        config: Arc<BundleConfig>,
    ) -> Result<ObjectStoreDir, BundlebaseError> {
        let url = str_to_url(path)?;
        Self::from_url(&url, config)
    }

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

    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Lists all files in the directory.
    pub async fn list_files(&self) -> Result<Vec<ObjectStoreFile>, BundlebaseError> {
        use futures::stream::StreamExt;

        let mut files = Vec::new();
        let mut list_iter = self.store.list(Some(&self.path));

        while let Some(meta_result) = list_iter.next().await {
            let location = meta_result?.location;
            // Get the relative path from self.path to location by stripping the prefix
            let location_str = location.as_ref();
            let prefix_str = self.path.as_ref();
            let relative_path = if let Some(stripped) = location_str.strip_prefix(prefix_str) {
                stripped.trim_start_matches('/')
            } else {
                location_str
            };
            files.push(ObjectStoreFile::new(
                &join_url(&self.url, relative_path)?,
                self.store.clone(),
                &location,
            )?)
        }
        Ok(files)
    }

    /// Returns a new directory object representing a subdirectory of this directory.
    /// If passed subdir starts with a "/", it's still treated as a relative path.
    pub fn subdir(&self, subdir: &str) -> Result<ObjectStoreDir, BundlebaseError> {
        Ok(ObjectStoreDir {
            url: join_url(&self.url, subdir)?,
            store: self.store.clone(),
            path: join_path(&self.path, subdir)?,
            config: self.config.clone(),
        })
    }

    pub fn file(&self, path: &str) -> Result<ObjectStoreFile, BundlebaseError> {
        let file_url = join_url(&self.url, path)?;
        let config_map = self.config.get_config_for_url(&file_url);
        let (store, object_path) = parse_url(&file_url, &config_map)?;

        ObjectStoreFile::new(&file_url, store, &object_path)
    }

    /// Creates a memory-backed directory for storing index and metadata files
    pub fn new_memory() -> Result<ObjectStoreDir, BundlebaseError> {
        let url = Url::parse("memory:///_indexes")?;
        let config = HashMap::new();
        let (store, path) = parse_url(&url, &config)?;
        ObjectStoreDir::new(&url, store, &path, BundleConfig::default().into())
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
        Ok(file_url(path))
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
        let dir = ObjectStoreDir::from_str(input, BundleConfig::default().into()).unwrap();
        assert_eq!(dir.url.to_string(), input);
        assert_eq!(dir.path.to_string(), expected_path);
    }

    #[test]
    fn test_from_string_complex() {
        assert!(
            ObjectStoreDir::from_str("memory://bucket/test", BundleConfig::default().into())
                .is_err(),
            "Memory must start with :///"
        );

        let dir =
            ObjectStoreDir::from_str("memory:///test/../test2", BundleConfig::default().into())
                .unwrap();
        assert_eq!(dir.path.to_string(), "test2");
        assert_eq!(dir.url.to_string(), "memory:///test2");

        let dir =
            ObjectStoreDir::from_str("relative/path", BundleConfig::default().into()).unwrap();
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
        let dir = ObjectStoreDir::from_url(&base, BundleConfig::default().into()).unwrap();
        let subdir = dir.subdir(subdir).unwrap();
        assert_eq!(subdir.url, expected_url);
        assert_eq!(subdir.path.to_string(), expected_path);
    }

    #[test]
    fn test_file() {
        let dir =
            ObjectStoreDir::from_str("memory:///test", BundleConfig::default().into()).unwrap();
        let file = dir.file("other").unwrap();
        assert_eq!(file.url().to_string(), "memory:///test/other");

        let file = dir.subdir("this/file.txt").unwrap();
        assert_eq!(file.url().to_string(), "memory:///test/this/file.txt");
    }

    #[tokio::test]
    async fn test_list_files() {
        let dir =
            ObjectStoreDir::from_str("memory:///test", BundleConfig::default().into()).unwrap();
        assert_eq!(0, dir.list_files().await.unwrap().len())
    }

    #[tokio::test]
    async fn test_null_url() {
        let dir = ObjectStoreDir::from_str(EMPTY_URL, BundleConfig::default().into()).unwrap();
        assert_eq!(0, dir.list_files().await.unwrap().len());
    }
}
