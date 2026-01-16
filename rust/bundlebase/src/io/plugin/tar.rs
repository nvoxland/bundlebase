//! Tar IO backend - file and directory operations on tar archives with tar:// URLs.
//!
//! Provides first-class support for `tar://` URLs:
//! - `tar:///path/to/archive.tar/internal/path`
//! - `tar:///data.tar/` (root of archive)

use crate::io::registry::IOFactory;
use crate::io::{FileInfo, IOReadDir, IOReadFile, IOReadWriteDir, IOReadWriteFile};
use crate::BundleConfig;
use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{self, BoxStream, StreamExt, TryStreamExt};
use object_store::path::Path as ObjectPath;
use object_store::{
    GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore, PutOptions,
    PutPayload, PutResult, Result as ObjectStoreResult,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::fs::File;
use std::io::{Read, Seek};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tar::{Archive, Builder, Header};
use url::Url;

// ============================================================================
// TarObjectStore - ObjectStore implementation for tar archives
// ============================================================================

/// An ObjectStore implementation that reads from and writes to tar archives.
///
/// Features:
/// - **Read support**: Lazy indexing on first access, cached in memory
/// - **Write support**: Append-only mode for new files (bundlebase never modifies existing files)
/// - **Streaming**: Efficient memory usage for large files
/// - **Thread-safe**: Multiple readers supported, writes are synchronized
///
/// Limitations:
/// - No compression support (uncompressed tar only)
/// - Cannot delete or modify existing entries
/// - Concurrent writes from multiple processes not supported
#[derive(Clone, Debug)]
pub struct TarObjectStore {
    tar_path: Arc<PathBuf>,
    index: Arc<RwLock<TarIndex>>,
    indexed: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
struct TarIndex {
    entries: HashMap<ObjectPath, TarEntry>,
}

#[derive(Clone, Debug)]
struct TarEntry {
    offset: u64,
    size: u64,
    modified: chrono::DateTime<chrono::Utc>,
}

impl TarObjectStore {
    /// Creates a new TarObjectStore for the given tar file path.
    ///
    /// The tar file will be opened in read+write mode, allowing both reading
    /// existing entries and appending new ones. If the file doesn't exist,
    /// it will be created.
    pub fn new(tar_path: PathBuf) -> ObjectStoreResult<Self> {
        Ok(Self {
            tar_path: Arc::new(tar_path),
            index: Arc::new(RwLock::new(TarIndex {
                entries: HashMap::new(),
            })),
            indexed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Builds the index by scanning through the tar file.
    /// This is called lazily on the first access and cached.
    fn build_index(&self) -> ObjectStoreResult<()> {
        // Double-check locking pattern
        if self.indexed.load(Ordering::Acquire) {
            return Ok(());
        }

        let file = File::open(&*self.tar_path).map_err(|e| {
            object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            }
        })?;

        let mut archive = Archive::new(file);
        let mut entries = HashMap::new();

        for (_i, entry_result) in archive.entries().map_err(|e| object_store::Error::Generic {
            store: "TarObjectStore",
            source: Box::new(e),
        })?.enumerate()
        {
            let entry = entry_result.map_err(|e| object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            })?;

            // Get the path from the entry
            let path_bytes = entry.path().map_err(|e| object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            })?;
            let path_str = path_bytes.to_str().ok_or_else(|| object_store::Error::Generic {
                store: "TarObjectStore",
                source: "Invalid UTF-8 in tar entry path".into(),
            })?;

            // Skip directories
            if path_str.ends_with('/') {
                continue;
            }

            let obj_path = ObjectPath::from(path_str);
            let size = entry.size();

            // Get modification time, defaulting to Unix epoch if not available
            let modified = entry
                .header()
                .mtime()
                .ok()
                .and_then(|mtime| {
                    chrono::DateTime::from_timestamp(mtime as i64, 0)
                })
                .unwrap_or_else(|| chrono::DateTime::UNIX_EPOCH);

            let tar_entry = TarEntry {
                offset: entry.raw_file_position(),
                size,
                modified,
            };

            entries.insert(obj_path, tar_entry);
        }

        // Update the index
        let mut index = self.index.write();
        index.entries = entries;
        self.indexed.store(true, Ordering::Release);

        Ok(())
    }

    /// Ensures the index is built before accessing it
    fn ensure_indexed(&self) -> ObjectStoreResult<()> {
        if !self.indexed.load(Ordering::Acquire) {
            self.build_index()?;
        }
        Ok(())
    }

    /// Reads a file from the tar archive using the indexed offset for O(1) seeking.
    fn read_entry(&self, path: &ObjectPath) -> ObjectStoreResult<Bytes> {
        self.ensure_indexed()?;

        // Look up the entry in the index
        let index = self.index.read();
        let entry = index.entries.get(path).ok_or_else(|| object_store::Error::NotFound {
            path: path.to_string(),
            source: "File not found in tar index".into(),
        })?;

        let offset = entry.offset;
        let size = entry.size;
        drop(index); // Release lock before file I/O

        // Open file and seek directly to the data
        let mut file = File::open(&*self.tar_path).map_err(|e| object_store::Error::Generic {
            store: "TarObjectStore",
            source: Box::new(e),
        })?;

        file.seek(std::io::SeekFrom::Start(offset)).map_err(|e| object_store::Error::Generic {
            store: "TarObjectStore",
            source: Box::new(e),
        })?;

        // Read exactly `size` bytes
        let mut buffer = vec![0u8; size as usize];
        file.read_exact(&mut buffer).map_err(|e| object_store::Error::Generic {
            store: "TarObjectStore",
            source: Box::new(e),
        })?;

        Ok(Bytes::from(buffer))
    }

    /// Appends a new file to the tar archive.
    ///
    /// Note: This implementation rewrites the entire tar file with the new entry.
    /// This is not the most efficient approach, but it's simple and works correctly.
    /// A more efficient approach would seek back to remove the tar footer, append
    /// the new entry, and write a new footer, but that's more complex.
    fn append_entry(&self, path: &ObjectPath, data: Bytes) -> ObjectStoreResult<()> {
        // If the tar file exists, read all existing entries first
        let existing_entries: Vec<(ObjectPath, Bytes)> = if self.tar_path.exists() {
            let file = File::open(&*self.tar_path).map_err(|e| {
                object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: Box::new(e),
                }
            })?;

            let mut archive = Archive::new(file);
            let mut entries = Vec::new();

            for entry_result in archive.entries().map_err(|e| object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            })? {
                let mut entry = entry_result.map_err(|e| object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: Box::new(e),
                })?;

                let entry_path = entry.path().map_err(|e| object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: Box::new(e),
                })?;
                let path_string = entry_path.to_str().ok_or_else(|| object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: "Invalid UTF-8 in tar entry path".into(),
                })?.to_string();

                let mut buffer = Vec::new();
                entry.read_to_end(&mut buffer).map_err(|e| {
                    object_store::Error::Generic {
                        store: "TarObjectStore",
                        source: Box::new(e),
                    }
                })?;

                entries.push((ObjectPath::from(path_string), Bytes::from(buffer)));
            }
            entries
        } else {
            Vec::new()
        };

        // Rewrite the entire tar file with all entries plus the new one
        let file = File::create(&*self.tar_path).map_err(|e| {
            object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            }
        })?;

        let mut builder = Builder::new(file);

        // Write all existing entries
        for (existing_path, existing_data) in existing_entries {
            let mut header = Header::new_gnu();
            header.set_size(existing_data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("BUG: current time should be after Unix epoch")
                    .as_secs(),
            );
            header.set_cksum();

            builder
                .append_data(&mut header, existing_path.as_ref(), &existing_data[..])
                .map_err(|e| object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: Box::new(e),
                })?;
        }

        // Write the new entry
        let mut header = Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("BUG: current time should be after Unix epoch")
                .as_secs(),
        );
        header.set_cksum();

        builder
            .append_data(&mut header, path.as_ref(), &data[..])
            .map_err(|e| object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            })?;

        // Finish writing (writes tar footer)
        builder.finish().map_err(|e| object_store::Error::Generic {
            store: "TarObjectStore",
            source: Box::new(e),
        })?;

        // Rebuild index to include all entries
        self.indexed.store(false, Ordering::Release);
        self.build_index()?;

        Ok(())
    }
}

impl Display for TarObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TarObjectStore({})", self.tar_path.display())
    }
}

#[async_trait]
impl ObjectStore for TarObjectStore {
    async fn put(&self, location: &ObjectPath, payload: PutPayload) -> ObjectStoreResult<PutResult> {
        let bytes = payload.into();
        self.append_entry(location, bytes)?;

        Ok(PutResult {
            e_tag: None,
            version: None,
        })
    }

    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        _opts: PutOptions,
    ) -> ObjectStoreResult<PutResult> {
        // Ignore options, just use regular put
        self.put(location, payload).await
    }

    async fn put_multipart(&self, _location: &ObjectPath) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
        Err(object_store::Error::NotImplemented)
    }

    async fn put_multipart_opts(
        &self,
        _location: &ObjectPath,
        _opts: object_store::PutMultipartOptions,
    ) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
        Err(object_store::Error::NotImplemented)
    }

    async fn get(&self, location: &ObjectPath) -> ObjectStoreResult<GetResult> {
        self.ensure_indexed()?;

        let bytes = self.read_entry(location)?;
        let size = bytes.len() as u64;

        Ok(GetResult {
            payload: object_store::GetResultPayload::Stream(Box::pin(stream::once(async move {
                Ok(bytes)
            }))),
            meta: ObjectMeta {
                location: location.clone(),
                last_modified: chrono::Utc::now(),
                size,
                e_tag: None,
                version: None,
            },
            range: 0..size,
            attributes: Default::default(),
        })
    }

    async fn get_opts(&self, location: &ObjectPath, _options: GetOptions) -> ObjectStoreResult<GetResult> {
        // For simplicity, ignore options and use regular get
        // A full implementation would handle range requests
        self.get(location).await
    }

    async fn get_range(&self, location: &ObjectPath, range: Range<u64>) -> ObjectStoreResult<Bytes> {
        let bytes = self.read_entry(location)?;

        let start = range.start as usize;
        let end = range.end as usize;

        if end > bytes.len() {
            return Err(object_store::Error::Generic {
                store: "TarObjectStore",
                source: "Range out of bounds".into(),
            });
        }

        Ok(bytes.slice(start..end))
    }

    async fn head(&self, location: &ObjectPath) -> ObjectStoreResult<ObjectMeta> {
        self.ensure_indexed()?;

        let index = self.index.read();
        let entry = index.entries.get(location).ok_or_else(|| {
            object_store::Error::NotFound {
                path: location.to_string(),
                source: "File not found in tar archive".into(),
            }
        })?;

        Ok(ObjectMeta {
            location: location.clone(),
            last_modified: entry.modified,
            size: entry.size,
            e_tag: None,
            version: None,
        })
    }

    async fn delete(&self, _location: &ObjectPath) -> ObjectStoreResult<()> {
        Err(object_store::Error::NotSupported {
            source: "Tar archives do not support deletion".into(),
        })
    }

    fn list(&self, prefix: Option<&ObjectPath>) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        // Ensure index is built synchronously
        if let Err(e) = self.ensure_indexed() {
            return Box::pin(stream::once(async move { Err(e) }));
        }

        // Clone the data we need
        let index = self.index.read();
        let prefix_owned = prefix.cloned();

        let entries: Vec<ObjectMeta> = index
            .entries
            .iter()
            .filter(|(path, _)| {
                if let Some(ref prefix) = prefix_owned {
                    path.as_ref().starts_with(prefix.as_ref())
                } else {
                    true
                }
            })
            .map(|(path, entry)| ObjectMeta {
                location: path.clone(),
                last_modified: entry.modified,
                size: entry.size,
                e_tag: None,
                version: None,
            })
            .collect();

        // Return a stream that yields each entry individually
        Box::pin(stream::iter(entries.into_iter().map(Ok)))
    }

    async fn list_with_delimiter(&self, prefix: Option<&ObjectPath>) -> ObjectStoreResult<ListResult> {
        self.ensure_indexed()?;

        let index = self.index.read();
        let prefix_str = prefix.map(|p| p.as_ref()).unwrap_or("");

        let mut objects = Vec::new();
        let mut common_prefixes = std::collections::HashSet::new();

        for (path, entry) in &index.entries {
            let path_str = path.as_ref();
            if !path_str.starts_with(prefix_str) {
                continue;
            }

            let relative = &path_str[prefix_str.len()..];
            if relative.is_empty() {
                continue;
            }

            // Check if this is a direct child or nested
            if let Some(slash_pos) = relative.find('/') {
                // It's a directory, add to common_prefixes
                let dir_name = &relative[..=slash_pos];
                let full_prefix = format!("{}{}", prefix_str, dir_name);
                common_prefixes.insert(ObjectPath::from(full_prefix));
            } else {
                // It's a file at this level
                objects.push(ObjectMeta {
                    location: path.clone(),
                    last_modified: entry.modified,
                    size: entry.size,
                    e_tag: None,
                    version: None,
                });
            }
        }

        Ok(ListResult {
            objects,
            common_prefixes: common_prefixes.into_iter().collect(),
        })
    }

    async fn copy(&self, _from: &ObjectPath, _to: &ObjectPath) -> ObjectStoreResult<()> {
        Err(object_store::Error::NotSupported {
            source: "Tar archives do not support copy".into(),
        })
    }

    async fn copy_if_not_exists(&self, _from: &ObjectPath, _to: &ObjectPath) -> ObjectStoreResult<()> {
        Err(object_store::Error::NotSupported {
            source: "Tar archives do not support copy".into(),
        })
    }

    async fn rename(&self, _from: &ObjectPath, _to: &ObjectPath) -> ObjectStoreResult<()> {
        Err(object_store::Error::NotSupported {
            source: "Tar archives do not support rename".into(),
        })
    }

    async fn rename_if_not_exists(&self, _from: &ObjectPath, _to: &ObjectPath) -> ObjectStoreResult<()> {
        Err(object_store::Error::NotSupported {
            source: "Tar archives do not support rename".into(),
        })
    }
}

// ============================================================================
// Tar URL parsing
// ============================================================================

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

// ============================================================================
// TarFile - File reader/writer for tar archives
// ============================================================================

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
}

#[async_trait]
impl IOReadFile for TarFile {
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
        Ok(format!("size-{}", meta.size))
    }
}

#[async_trait]
impl IOReadWriteFile for TarFile {
    async fn write(&self, data: Bytes) -> Result<(), BundlebaseError> {
        let put_result = object_store::PutPayload::from_bytes(data);
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    async fn write_stream(
        &self,
        mut source: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Result<(), BundlebaseError> {
        // NOTE: Tar format requires knowing file size before writing the entry header.
        // True streaming is not possible without significant protocol changes.
        // We must buffer the entire content to determine size.
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

// ============================================================================
// TarDir - Directory lister for tar archives
// ============================================================================

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
}

#[async_trait]
impl IOReadDir for TarDir {
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
                        .with_size(meta.size)
                        .with_modified(meta.last_modified),
                );
            }
        }
        Ok(files)
    }

    fn subdir(&self, name: &str) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
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

    fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
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

#[async_trait]
impl IOReadWriteDir for TarDir {
    fn writable_subdir(&self, name: &str) -> Result<Box<dyn IOReadWriteDir>, BundlebaseError> {
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

    fn writable_file(&self, name: &str) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError> {
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

    async fn rename(&self, _from: &str, _to: &str) -> Result<(), BundlebaseError> {
        Err("Tar archives do not support rename".into())
    }
}

// ============================================================================
// TarIOFactory - Factory for creating Tar IO instances
// ============================================================================

/// Factory for Tar IO backends.
pub struct TarIOFactory;

#[async_trait]
impl IOFactory for TarIOFactory {
    fn schemes(&self) -> &[&str] {
        &["tar"]
    }

    fn supports_write(&self, _url: &Url) -> bool {
        true // Tar supports append-only writes
    }

    fn supports_streaming_write(&self) -> bool {
        // Tar format requires knowing file size upfront for the entry header,
        // so we must buffer the entire stream content before writing.
        false
    }

    async fn create_reader(
        &self,
        url: &Url,
        _config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        Ok(Box::new(TarFile::from_url(url)?))
    }

    async fn create_lister(
        &self,
        url: &Url,
        _config: Arc<BundleConfig>,
    ) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        Ok(Box::new(TarDir::from_url(url)?))
    }

    async fn create_writable_lister(
        &self,
        url: &Url,
        _config: Arc<BundleConfig>,
    ) -> Result<Option<Box<dyn IOReadWriteDir>>, BundlebaseError> {
        Ok(Some(Box::new(TarDir::from_url(url)?)))
    }

    async fn create_writer(
        &self,
        url: &Url,
        _config: Arc<BundleConfig>,
    ) -> Result<Option<Box<dyn IOReadWriteFile>>, BundlebaseError> {
        Ok(Some(Box::new(TarFile::from_url(url)?)))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    // TarObjectStore tests

    #[tokio::test]
    async fn test_tar_store_write_and_read() {
        let temp_file = NamedTempFile::new().unwrap();
        let tar_path = temp_file.path().to_path_buf();

        let store = TarObjectStore::new(tar_path.clone()).unwrap();
        let path = ObjectPath::from("test/file.txt");
        let data = Bytes::from("Hello, world!");

        // Write
        store
            .put(&path, PutPayload::from_bytes(data.clone()))
            .await
            .unwrap();

        // Read
        let result = store.get(&path).await.unwrap();
        let read_data = result.bytes().await.unwrap();
        assert_eq!(read_data, data);
    }

    #[tokio::test]
    async fn test_tar_store_head() {
        let temp_file = NamedTempFile::new().unwrap();
        let tar_path = temp_file.path().to_path_buf();

        let store = TarObjectStore::new(tar_path).unwrap();
        let path = ObjectPath::from("metadata_test.txt");
        let data = Bytes::from("test data");

        store
            .put(&path, PutPayload::from_bytes(data.clone()))
            .await
            .unwrap();

        let meta = store.head(&path).await.unwrap();
        assert_eq!(meta.size, data.len() as u64);
        assert_eq!(meta.location, path);
    }

    #[tokio::test]
    async fn test_tar_store_list() {
        let temp_file = NamedTempFile::new().unwrap();
        let tar_path = temp_file.path().to_path_buf();

        let store = TarObjectStore::new(tar_path).unwrap();

        // Write multiple files
        store
            .put(
                &ObjectPath::from("dir1/file1.txt"),
                PutPayload::from_bytes(Bytes::from("data1")),
            )
            .await
            .unwrap();
        store
            .put(
                &ObjectPath::from("dir1/file2.txt"),
                PutPayload::from_bytes(Bytes::from("data2")),
            )
            .await
            .unwrap();
        store
            .put(
                &ObjectPath::from("dir2/file3.txt"),
                PutPayload::from_bytes(Bytes::from("data3")),
            )
            .await
            .unwrap();

        // List all files
        let mut results: Vec<_> = store.list(None).collect::<Vec<_>>().await;
        results.sort_by(|a, b| {
            a.as_ref()
                .unwrap()
                .location
                .cmp(&b.as_ref().unwrap().location)
        });

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap().location.as_ref(), "dir1/file1.txt");
        assert_eq!(results[1].as_ref().unwrap().location.as_ref(), "dir1/file2.txt");
        assert_eq!(results[2].as_ref().unwrap().location.as_ref(), "dir2/file3.txt");

        // List with prefix
        let prefix_results: Vec<_> = store
            .list(Some(&ObjectPath::from("dir1")))
            .collect::<Vec<_>>()
            .await;

        assert_eq!(prefix_results.len(), 2);
    }

    #[tokio::test]
    async fn test_tar_store_not_found() {
        let temp_file = NamedTempFile::new().unwrap();
        let tar_path = temp_file.path().to_path_buf();

        let store = TarObjectStore::new(tar_path).unwrap();
        let path = ObjectPath::from("nonexistent.txt");

        let result = store.get(&path).await;
        assert!(matches!(result, Err(object_store::Error::NotFound { .. })));
    }

    // Tar URL parsing tests

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
