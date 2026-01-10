//! Tar IO backend - file and directory operations on tar archives with tar:// URLs.
//!
//! Provides first-class support for `tar://` URLs:
//! - `tar:///path/to/archive.tar/internal/path`
//! - `tar:///data.tar/` (root of archive)

use crate::io::io_registry::IOFactory;
use crate::io::io_traits::{FileInfo, IOLister, IOReader, IOWriter};
use crate::io::TarObjectStore;
use crate::BundleConfig;
use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt, TryStreamExt};
use object_store::path::Path as ObjectPath;
use object_store::ObjectStore;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;

/// Parse a tar:// URL into archive path and internal path.
///
/// # URL Format
/// `tar:///<path-to-archive.tar>/<internal-path>`
///
/// Examples:
/// - `tar:///home/user/data.tar/subdir/file.parquet`
/// - `tar:///data.tar/` (root of archive)
///
/// # Returns
/// Tuple of (archive_path, internal_path)
pub fn parse_tar_url(url: &Url) -> Result<(PathBuf, String), BundlebaseError> {
    if url.scheme() != "tar" {
        return Err(format!("Expected 'tar' URL scheme, got '{}'", url.scheme()).into());
    }

    let full_path = url.path();
    if full_path.is_empty() || full_path == "/" {
        return Err("tar:// URL must include a path to a .tar file".into());
    }

    // Find the .tar extension to split archive path from internal path
    let tar_idx = full_path
        .find(".tar/")
        .or_else(|| {
            // Check if the path ends with .tar (no internal path)
            if full_path.ends_with(".tar") {
                Some(full_path.len() - 4)
            } else {
                None
            }
        })
        .ok_or_else(|| BundlebaseError::from("tar:// URL must contain .tar in path"))?;

    let archive_path = PathBuf::from(&full_path[..tar_idx + 4]); // Include .tar
    let internal_path = full_path
        .get(tar_idx + 5..)
        .unwrap_or("")
        .trim_start_matches('/')
        .to_string();

    Ok((archive_path, internal_path))
}

/// Tar file reader/writer - access to a single file within a tar archive.
pub struct TarFile {
    url: Url,
    store: Arc<TarObjectStore>,
    path: ObjectPath,
}

impl Debug for TarFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TarFile")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl TarFile {
    /// Create a TarFile from a tar:// URL.
    pub fn from_url(url: &Url) -> Result<Self, BundlebaseError> {
        let (archive_path, internal_path) = parse_tar_url(url)?;
        let store = Arc::new(TarObjectStore::new(archive_path)?);
        let path = ObjectPath::from(internal_path.as_str());

        Ok(Self {
            url: url.clone(),
            store,
            path,
        })
    }

    /// Create a TarFile with an existing store.
    pub fn new(url: Url, store: Arc<TarObjectStore>, path: ObjectPath) -> Self {
        Self { url, store, path }
    }

    /// Get the filename portion of the path.
    pub fn filename(&self) -> &str {
        self.path.filename().unwrap_or("")
    }
}

#[async_trait]
impl IOReader for TarFile {
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
        Ok(format!("size-{}", meta.size))
    }
}

#[async_trait]
impl IOWriter for TarFile {
    async fn write(&self, data: Bytes) -> Result<(), BundlebaseError> {
        let put_result = object_store::PutPayload::from_bytes(data);
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    async fn write_stream_boxed(
        &self,
        mut source: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Result<(), BundlebaseError> {
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
        // Tar archives don't support deletion
        Err("Tar archives do not support file deletion".into())
    }
}

/// Tar directory lister - access to list files within a tar archive.
pub struct TarDir {
    url: Url,
    store: Arc<TarObjectStore>,
    path: ObjectPath,
    archive_path: PathBuf,
}

impl Debug for TarDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TarDir")
            .field("url", &self.url)
            .field("path", &self.path)
            .finish()
    }
}

impl TarDir {
    /// Create a TarDir from a tar:// URL.
    pub fn from_url(url: &Url) -> Result<Self, BundlebaseError> {
        let (archive_path, internal_path) = parse_tar_url(url)?;
        let store = Arc::new(TarObjectStore::new(archive_path.clone())?);
        let path = ObjectPath::from(internal_path.as_str());

        Ok(Self {
            url: url.clone(),
            store,
            path,
            archive_path,
        })
    }

    /// Create a TarDir with an existing store.
    pub fn new(url: Url, store: Arc<TarObjectStore>, path: ObjectPath, archive_path: PathBuf) -> Self {
        Self {
            url,
            store,
            path,
            archive_path,
        }
    }
}

#[async_trait]
impl IOLister for TarDir {
    fn url(&self) -> &Url {
        &self.url
    }

    async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError> {
        let mut files = Vec::new();
        let mut list_iter = self.store.list(Some(&self.path));

        while let Some(meta_result) = list_iter.next().await {
            let meta = meta_result?;
            let location = meta.location;

            // Get the relative path
            let location_str = location.as_ref();
            let prefix_str = self.path.as_ref();
            let relative_path = if let Some(stripped) = location_str.strip_prefix(prefix_str) {
                stripped.trim_start_matches('/')
            } else {
                location_str
            };

            // Construct tar:// URL for file
            let file_url = format!(
                "tar://{}/{}",
                self.archive_path.display(),
                if relative_path.is_empty() {
                    location_str.to_string()
                } else {
                    format!("{}/{}", prefix_str.trim_end_matches('/'), relative_path)
                }
            );

            if let Ok(url) = Url::parse(&file_url) {
                files.push(
                    FileInfo::new(url)
                        .with_size(meta.size as u64)
                        .with_modified(meta.last_modified),
                );
            }
        }
        Ok(files)
    }

    fn subdir(&self, name: &str) -> Result<Box<dyn IOLister>, BundlebaseError> {
        let new_path = if self.path.as_ref().is_empty() {
            ObjectPath::from(name.trim_start_matches('/'))
        } else {
            self.path.child(name.trim_start_matches('/'))
        };

        let new_url = Url::parse(&format!(
            "tar://{}/{}",
            self.archive_path.display(),
            new_path.as_ref()
        ))?;

        Ok(Box::new(TarDir {
            url: new_url,
            store: self.store.clone(),
            path: new_path,
            archive_path: self.archive_path.clone(),
        }))
    }

    fn file(&self, name: &str) -> Result<Box<dyn IOReader>, BundlebaseError> {
        let new_path = if self.path.as_ref().is_empty() {
            ObjectPath::from(name.trim_start_matches('/'))
        } else {
            self.path.child(name.trim_start_matches('/'))
        };

        let new_url = Url::parse(&format!(
            "tar://{}/{}",
            self.archive_path.display(),
            new_path.as_ref()
        ))?;

        Ok(Box::new(TarFile::new(new_url, self.store.clone(), new_path)))
    }
}

/// Factory for Tar IO backends.
pub struct TarIOFactory;

#[async_trait]
impl IOFactory for TarIOFactory {
    fn schemes(&self) -> &[&str] {
        &["tar"]
    }

    fn supports_write(&self) -> bool {
        true // Tar supports append-only writes
    }

    async fn create_reader(
        &self,
        url: &Url,
        _config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReader>, BundlebaseError> {
        Ok(Box::new(TarFile::from_url(url)?))
    }

    async fn create_lister(
        &self,
        url: &Url,
        _config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOLister>, BundlebaseError> {
        Ok(Box::new(TarDir::from_url(url)?))
    }

    async fn create_writer(
        &self,
        url: &Url,
        _config: Arc<BundleConfig>,
    ) -> Result<Option<Box<dyn IOWriter>>, BundlebaseError> {
        Ok(Some(Box::new(TarFile::from_url(url)?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tar_url_with_internal_path() {
        let url = Url::parse("tar:///home/user/data.tar/subdir/file.parquet").unwrap();
        let (archive_path, internal_path) = parse_tar_url(&url).unwrap();
        assert_eq!(archive_path, PathBuf::from("/home/user/data.tar"));
        assert_eq!(internal_path, "subdir/file.parquet");
    }

    #[test]
    fn test_parse_tar_url_root() {
        let url = Url::parse("tar:///data.tar/").unwrap();
        let (archive_path, internal_path) = parse_tar_url(&url).unwrap();
        assert_eq!(archive_path, PathBuf::from("/data.tar"));
        assert_eq!(internal_path, "");
    }

    #[test]
    fn test_parse_tar_url_no_internal_path() {
        let url = Url::parse("tar:///archive.tar").unwrap();
        let (archive_path, internal_path) = parse_tar_url(&url).unwrap();
        assert_eq!(archive_path, PathBuf::from("/archive.tar"));
        assert_eq!(internal_path, "");
    }

    #[test]
    fn test_parse_tar_url_wrong_scheme() {
        let url = Url::parse("file:///data.tar").unwrap();
        let result = parse_tar_url(&url);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 'tar'"));
    }

    #[test]
    fn test_parse_tar_url_no_tar_extension() {
        let url = Url::parse("tar:///data/file.txt").unwrap();
        let result = parse_tar_url(&url);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must contain .tar"));
    }
}
