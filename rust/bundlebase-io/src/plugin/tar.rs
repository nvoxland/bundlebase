//! Tar IO backend - file and directory operations on tar archives with tar+file:// URLs.
//!
//! Provides first-class support for `tar+<scheme>://` URLs:
//! - `tar+file:///path/to/archive.tar/internal/path` (read+write, local)
//! - `tar+s3://bucket/archive.tar/internal/path` (read-only, remote)
//! - `tar+gs://bucket/archive.tar/internal/path` (read-only, remote)
//! - `tar+azure://container/archive.tar/internal/path` (read-only, remote)
//!
//! Remote tar archives must contain a `_bundlebase_manifest.json` as the first entry
//! (written by `export_tar()`). This enables O(1) startup via two range requests.

use crate::registry::IOFactory;
use crate::BundlebaseError;
use crate::ConfigProvider;
use crate::{FileInfo, IOReadDir, IOReadFile, IOReadWriteDir, IOReadWriteFile};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{self, BoxStream, StreamExt, TryStreamExt};
use futures::FutureExt;
use object_store::path::Path as ObjectPath;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutOptions, PutPayload, PutResult, Result as ObjectStoreResult,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tar::{Archive, Builder, Header};
use url::Url;

/// Name of the manifest file embedded as the first tar entry by `export_tar()`.
pub const TAR_MANIFEST_FILENAME: &str = "_bundlebase_manifest.json";

// ============================================================================
// TarObjectStore - Local read+write ObjectStore for tar archives
// ============================================================================

/// An ObjectStore implementation that reads from and writes to local tar archives.
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
    ///
    /// A missing tar file is treated as an empty archive — this matches the
    /// "create new tar bundle" path where `BundleBuilder::create()` runs an
    /// existence check on the META directory before any file has been written.
    fn build_index(&self) -> ObjectStoreResult<()> {
        // Double-check locking pattern
        if self.indexed.load(Ordering::Acquire) {
            return Ok(());
        }

        let file = match File::open(&*self.tar_path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Treat as empty archive; the file will be created on first write.
                let mut index = self.index.write();
                index.entries = HashMap::new();
                self.indexed.store(true, Ordering::Release);
                return Ok(());
            }
            Err(e) => {
                return Err(object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: Box::new(e),
                })
            }
        };

        let mut archive = Archive::new(file);
        let mut entries = HashMap::new();

        for (_i, entry_result) in archive
            .entries()
            .map_err(|e| object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            })?
            .enumerate()
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
            let path_str = path_bytes
                .to_str()
                .ok_or_else(|| object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: "Invalid UTF-8 in tar entry path".into(),
                })?;

            // Skip directories and the bundlebase manifest (internal metadata)
            if path_str.ends_with('/') || path_str == TAR_MANIFEST_FILENAME {
                continue;
            }

            let obj_path = ObjectPath::from(path_str);
            let size = entry.size();

            // Get modification time, defaulting to Unix epoch if not available
            let modified = entry
                .header()
                .mtime()
                .ok()
                .and_then(|mtime| chrono::DateTime::from_timestamp(mtime as i64, 0))
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
        let entry = index
            .entries
            .get(path)
            .ok_or_else(|| object_store::Error::NotFound {
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

        file.seek(std::io::SeekFrom::Start(offset))
            .map_err(|e| object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            })?;

        // Read exactly `size` bytes
        let mut buffer = vec![0u8; size as usize];
        file.read_exact(&mut buffer)
            .map_err(|e| object_store::Error::Generic {
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
            let file = File::open(&*self.tar_path).map_err(|e| object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            })?;

            let mut archive = Archive::new(file);
            let mut entries = Vec::new();

            for entry_result in archive
                .entries()
                .map_err(|e| object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: Box::new(e),
                })?
            {
                let mut entry = entry_result.map_err(|e| object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: Box::new(e),
                })?;

                let entry_path = entry.path().map_err(|e| object_store::Error::Generic {
                    store: "TarObjectStore",
                    source: Box::new(e),
                })?;
                let path_string = entry_path
                    .to_str()
                    .ok_or_else(|| object_store::Error::Generic {
                        store: "TarObjectStore",
                        source: "Invalid UTF-8 in tar entry path".into(),
                    })?
                    .to_string();

                let mut buffer = Vec::new();
                entry
                    .read_to_end(&mut buffer)
                    .map_err(|e| object_store::Error::Generic {
                        store: "TarObjectStore",
                        source: Box::new(e),
                    })?;

                entries.push((ObjectPath::from(path_string), Bytes::from(buffer)));
            }
            entries
        } else {
            Vec::new()
        };

        // Rewrite the entire tar file with all entries plus the new one
        let file = File::create(&*self.tar_path).map_err(|e| object_store::Error::Generic {
            store: "TarObjectStore",
            source: Box::new(e),
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
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        _opts: PutOptions,
    ) -> ObjectStoreResult<PutResult> {
        let bytes: Bytes = payload.into();
        self.append_entry(location, bytes)?;

        Ok(PutResult {
            e_tag: None,
            version: None,
        })
    }

    async fn put_multipart_opts(
        &self,
        _location: &ObjectPath,
        _opts: object_store::PutMultipartOptions,
    ) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
        Err(object_store::Error::NotImplemented {
            operation: "put_multipart_opts".to_string(),
            implementer: "TarObjectStore".to_string(),
        })
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        _options: GetOptions,
    ) -> ObjectStoreResult<GetResult> {
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

    fn delete_stream(
        &self,
        locations: BoxStream<'static, ObjectStoreResult<ObjectPath>>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectPath>> {
        Box::pin(locations.then(|_| async {
            Err(object_store::Error::NotSupported {
                source: "Tar archives do not support deletion".into(),
            })
        }))
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
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

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> ObjectStoreResult<ListResult> {
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

    async fn copy_opts(
        &self,
        _from: &ObjectPath,
        _to: &ObjectPath,
        _options: CopyOptions,
    ) -> ObjectStoreResult<()> {
        Err(object_store::Error::NotSupported {
            source: "Tar archives do not support copy".into(),
        })
    }
}

// ============================================================================
// ReadOnlyTarObjectStore - Remote read-only ObjectStore backed by any store
// ============================================================================

/// A read-only ObjectStore that reads individual files from a tar archive stored
/// in any `ObjectStore` (S3, GCS, Azure, etc.) using range requests.
///
/// Requires a `_bundlebase_manifest.json` as the first tar entry (written by
/// `export_tar()`). The manifest lists all entry names and sizes, enabling
/// O(1) byte-offset computation without scanning the entire archive.
///
/// Indexing requires exactly 2 range requests:
/// 1. `0..512` — parse the first tar header to learn manifest size
/// 2. `512..512+size` — read the manifest JSON, compute all offsets
///
/// Each subsequent file read is a single `get_range()` at the computed offset.
#[derive(Clone)]
pub struct ReadOnlyTarObjectStore {
    inner_store: Arc<dyn ObjectStore>,
    archive_path: ObjectPath,
    index: Arc<RwLock<TarIndex>>,
    indexed: Arc<AtomicBool>,
}

impl Debug for ReadOnlyTarObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadOnlyTarObjectStore")
            .field("archive_path", &self.archive_path)
            .finish()
    }
}

/// Round up to the next 512-byte boundary (tar block size).
fn pad512(size: u64) -> u64 {
    (size + 511) & !511
}

/// Compute byte offsets for all entries given the manifest data size and entry list.
///
/// Returns a map from entry name to (data_offset, data_size).
/// The layout is: manifest_header(512) + manifest_data(padded) + for each entry: header(512) + data(padded).
fn compute_offsets(
    manifest_data_size: u64,
    entries: &[(String, u64)],
) -> HashMap<ObjectPath, TarEntry> {
    let mut offset = 512 + pad512(manifest_data_size); // skip past manifest entry
    let mut result = HashMap::new();

    for (name, size) in entries {
        let data_offset = offset + 512; // skip this entry's header
        result.insert(
            ObjectPath::from(name.as_str()),
            TarEntry {
                offset: data_offset,
                size: *size,
                modified: chrono::DateTime::UNIX_EPOCH,
            },
        );
        offset = data_offset + pad512(*size);
    }

    result
}

impl ReadOnlyTarObjectStore {
    /// Create a new ReadOnlyTarObjectStore backed by the given object store.
    pub fn new(inner_store: Arc<dyn ObjectStore>, archive_path: ObjectPath) -> Self {
        Self {
            inner_store,
            archive_path,
            index: Arc::new(RwLock::new(TarIndex {
                entries: HashMap::new(),
            })),
            indexed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Build the index by reading the manifest from the first tar entry.
    /// This requires exactly 2 range requests to the backing store.
    async fn build_index(&self) -> ObjectStoreResult<()> {
        if self.indexed.load(Ordering::Acquire) {
            return Ok(());
        }

        // Request 1: Read the first 512 bytes (tar header of the manifest entry)
        let header_bytes = self
            .inner_store
            .get_range(&self.archive_path, 0..512)
            .await?;

        // Parse the tar header to get the entry name and size
        let mut header = Header::new_gnu();
        header.as_mut_bytes().copy_from_slice(&header_bytes[..512]);

        let entry_name = header.path().map_err(|e| object_store::Error::Generic {
            store: "ReadOnlyTarObjectStore",
            source: Box::new(e),
        })?;
        let entry_name_str = entry_name
            .to_str()
            .ok_or_else(|| object_store::Error::Generic {
                store: "ReadOnlyTarObjectStore",
                source: "Invalid UTF-8 in tar header path".into(),
            })?;

        if entry_name_str != TAR_MANIFEST_FILENAME {
            return Err(object_store::Error::Generic {
                store: "ReadOnlyTarObjectStore",
                source: format!(
                    "Remote tar archives require a bundlebase manifest. \
                     Expected first entry to be '{}', got '{}'. \
                     Use `export_tar()` to create a compatible archive.",
                    TAR_MANIFEST_FILENAME, entry_name_str
                )
                .into(),
            });
        }

        let manifest_size = header.size().map_err(|e| object_store::Error::Generic {
            store: "ReadOnlyTarObjectStore",
            source: Box::new(e),
        })?;

        // Request 2: Read the manifest data
        let manifest_bytes = self
            .inner_store
            .get_range(&self.archive_path, 512..512 + manifest_size)
            .await?;

        // Parse the manifest JSON: [{"name": "...", "size": N}, ...]
        let manifest: Vec<serde_json::Value> =
            serde_json::from_slice(&manifest_bytes).map_err(|e| object_store::Error::Generic {
                store: "ReadOnlyTarObjectStore",
                source: Box::new(e),
            })?;

        let entries_vec: Vec<(String, u64)> = manifest
            .iter()
            .map(|entry| {
                let name = entry["name"].as_str().unwrap_or_default().to_string();
                let size = entry["size"].as_u64().unwrap_or(0);
                (name, size)
            })
            .collect();

        let computed = compute_offsets(manifest_size, &entries_vec);

        let mut index = self.index.write();
        index.entries = computed;
        self.indexed.store(true, Ordering::Release);

        Ok(())
    }

    /// Ensure the index is built.
    async fn ensure_indexed(&self) -> ObjectStoreResult<()> {
        if !self.indexed.load(Ordering::Acquire) {
            self.build_index().await?;
        }
        Ok(())
    }

    /// Read an entry's data via a single range request.
    async fn read_entry(&self, path: &ObjectPath) -> ObjectStoreResult<Bytes> {
        self.ensure_indexed().await?;

        let (offset, size) = {
            let index = self.index.read();
            let entry = index
                .entries
                .get(path)
                .ok_or_else(|| object_store::Error::NotFound {
                    path: path.to_string(),
                    source: "File not found in remote tar index".into(),
                })?;
            (entry.offset, entry.size)
        };

        self.inner_store
            .get_range(&self.archive_path, offset..offset + size)
            .await
    }
}

impl Display for ReadOnlyTarObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReadOnlyTarObjectStore({})", self.archive_path)
    }
}

#[async_trait]
impl ObjectStore for ReadOnlyTarObjectStore {
    async fn put_opts(
        &self,
        _location: &ObjectPath,
        _payload: PutPayload,
        _opts: PutOptions,
    ) -> ObjectStoreResult<PutResult> {
        Err(object_store::Error::NotSupported {
            source: "Remote tar archives are read-only".into(),
        })
    }

    async fn put_multipart_opts(
        &self,
        _location: &ObjectPath,
        _opts: object_store::PutMultipartOptions,
    ) -> ObjectStoreResult<Box<dyn MultipartUpload>> {
        Err(object_store::Error::NotSupported {
            source: "Remote tar archives are read-only".into(),
        })
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        _options: GetOptions,
    ) -> ObjectStoreResult<GetResult> {
        let bytes = self.read_entry(location).await?;
        let size = bytes.len() as u64;

        Ok(GetResult {
            payload: object_store::GetResultPayload::Stream(Box::pin(stream::once(async move {
                Ok(bytes)
            }))),
            meta: ObjectMeta {
                location: location.clone(),
                last_modified: chrono::DateTime::UNIX_EPOCH,
                size,
                e_tag: None,
                version: None,
            },
            range: 0..size,
            attributes: Default::default(),
        })
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, ObjectStoreResult<ObjectPath>>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectPath>> {
        Box::pin(locations.then(|_| async {
            Err(object_store::Error::NotSupported {
                source: "Remote tar archives are read-only".into(),
            })
        }))
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, ObjectStoreResult<ObjectMeta>> {
        let this = self.clone();
        let prefix_owned = prefix.cloned();

        // Use a future that builds the index if needed, then streams entries
        let fut = async move {
            this.ensure_indexed().await?;

            let guard = this.index.read();
            let entries: Vec<ObjectMeta> = guard
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

            Ok::<_, object_store::Error>(stream::iter(entries.into_iter().map(Ok)))
        };

        Box::pin(fut.into_stream().try_flatten())
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> ObjectStoreResult<ListResult> {
        self.ensure_indexed().await?;

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

            if let Some(slash_pos) = relative.find('/') {
                let dir_name = &relative[..=slash_pos];
                let full_prefix = format!("{}{}", prefix_str, dir_name);
                common_prefixes.insert(ObjectPath::from(full_prefix));
            } else {
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

    async fn copy_opts(
        &self,
        _from: &ObjectPath,
        _to: &ObjectPath,
        _options: CopyOptions,
    ) -> ObjectStoreResult<()> {
        Err(object_store::Error::NotSupported {
            source: "Remote tar archives are read-only".into(),
        })
    }
}

// ============================================================================
// Tar URL parsing
// ============================================================================

/// Represents the location of a tar archive — either local or remote.
#[derive(Debug)]
pub enum TarArchiveLocation {
    /// Local filesystem path (read+write).
    Local(PathBuf),
    /// Remote object store (read-only). Contains the backing store and path to the .tar file.
    Remote {
        store: Arc<dyn ObjectStore>,
        path: ObjectPath,
    },
}

/// Parse a `tar+<scheme>://` URL into the archive location and internal path.
///
/// For `tar+file://` URLs, returns `Local` with a filesystem path.
/// For other `tar+*://` URLs (s3, gs, azure, etc.), strips the `tar+` prefix,
/// builds an ObjectStore for the inner scheme, and returns `Remote`.
///
/// # Returns
/// Tuple of (TarArchiveLocation, internal_path)
pub fn parse_tar_url(
    url: &Url,
    config: &dyn ConfigProvider,
) -> Result<(TarArchiveLocation, String), BundlebaseError> {
    let scheme = url.scheme();
    if !scheme.starts_with("tar+") {
        return Err(format!("Expected 'tar+<scheme>' URL scheme, got '{}'", scheme).into());
    }

    let full_path = url.path();
    if full_path.is_empty() || full_path == "/" {
        return Err("tar+<scheme>:// URL must include a path to a .tar file".into());
    }

    // Find the .tar extension to split archive path from internal path
    let tar_idx = full_path
        .find(".tar/")
        .or_else(|| {
            if full_path.ends_with(".tar") {
                Some(full_path.len() - 4)
            } else {
                None
            }
        })
        .ok_or_else(|| BundlebaseError::from("tar+<scheme>:// URL must contain .tar in path"))?;

    let archive_path_str = &full_path[..tar_idx + 4]; // Include .tar
    let internal_path = full_path
        .get(tar_idx + 5..)
        .unwrap_or("")
        .trim_start_matches('/')
        .to_string();

    let inner_scheme = &scheme[4..]; // strip "tar+"

    let location = if inner_scheme == "file" {
        TarArchiveLocation::Local(PathBuf::from(archive_path_str))
    } else {
        // Build the inner URL: e.g. "s3://bucket/path/to/archive.tar"
        // Reconstruct from the original URL but with the inner scheme and only the archive path
        let inner_url_str = format!(
            "{}:{}",
            inner_scheme,
            &url.as_str()[scheme.len() + 1..] // everything after "tar+<scheme>:"
        );
        // Parse to get the full URL, then truncate to just the archive path
        let inner_full_url = Url::parse(&inner_url_str)?;

        // Build a URL that points to just the archive .tar file
        let archive_url_str = format!(
            "{}://{}{}",
            inner_scheme,
            inner_full_url.authority(),
            archive_path_str
        );
        let archive_url = Url::parse(&archive_url_str)?;

        let store =
            super::object_store::build_object_store(&archive_url, &archive_url_str, config)?;
        let obj_path = ObjectPath::from(archive_path_str);

        TarArchiveLocation::Remote {
            store: Arc::from(store),
            path: obj_path,
        }
    };

    Ok((location, internal_path))
}

/// Construct the base tar URL string from the original URL and internal path.
/// E.g., for `tar+s3://bucket/data.tar/some/file`, returns `tar+s3://bucket/data.tar`.
fn base_tar_url(url: &Url, internal_path: &str) -> String {
    let full = url.as_str();
    if internal_path.is_empty() {
        // URL might or might not end with "/"
        full.trim_end_matches('/').to_string()
    } else {
        // Remove the "/internal_path" suffix
        let suffix = format!("/{}", internal_path);
        full.strip_suffix(&suffix).unwrap_or(full).to_string()
    }
}

// ============================================================================
// TarFile - File reader/writer for tar archives
// ============================================================================

/// Tar file reader/writer - access to a single file within a tar archive.
pub struct TarFile {
    url: Url,
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    writable: bool,
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
    /// Create a TarFile from a `tar+<scheme>://` URL.
    pub fn from_url(url: &Url, config: &dyn ConfigProvider) -> Result<Self, BundlebaseError> {
        let (location, internal_path) = parse_tar_url(url, config)?;
        let path = ObjectPath::from(internal_path.as_str());

        let (store, writable): (Arc<dyn ObjectStore>, bool) = match location {
            TarArchiveLocation::Local(tar_path) => (Arc::new(TarObjectStore::new(tar_path)?), true),
            TarArchiveLocation::Remote {
                store,
                path: archive_path,
            } => (
                Arc::new(ReadOnlyTarObjectStore::new(store, archive_path)),
                false,
            ),
        };

        Ok(Self {
            url: url.clone(),
            store,
            path,
            writable,
        })
    }

    /// Create a TarFile with an existing store.
    pub fn new(url: Url, store: Arc<dyn ObjectStore>, path: ObjectPath, writable: bool) -> Self {
        Self {
            url,
            store,
            path,
            writable,
        }
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

    async fn open_stream(
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
        if !self.writable {
            return Err("Remote tar archives are read-only".into());
        }
        let put_result = object_store::PutPayload::from_bytes(data);
        self.store.put(&self.path, put_result).await?;
        Ok(())
    }

    async fn write_stream(
        &self,
        mut source: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
    ) -> Result<(), BundlebaseError> {
        if !self.writable {
            return Err("Remote tar archives are read-only".into());
        }
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
    store: Arc<dyn ObjectStore>,
    path: ObjectPath,
    base_tar_url: String,
    writable: bool,
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
    /// Create a TarDir from a `tar+<scheme>://` URL.
    pub fn from_url(url: &Url, config: &dyn ConfigProvider) -> Result<Self, BundlebaseError> {
        let (location, internal_path) = parse_tar_url(url, config)?;
        let path = ObjectPath::from(internal_path.as_str());
        let tar_url = base_tar_url(url, &internal_path);

        let (store, writable): (Arc<dyn ObjectStore>, bool) = match location {
            TarArchiveLocation::Local(tar_path) => (Arc::new(TarObjectStore::new(tar_path)?), true),
            TarArchiveLocation::Remote {
                store,
                path: archive_path,
            } => (
                Arc::new(ReadOnlyTarObjectStore::new(store, archive_path)),
                false,
            ),
        };

        Ok(Self {
            url: url.clone(),
            store,
            path,
            base_tar_url: tar_url,
            writable,
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

            // Construct tar URL for file using base_tar_url
            let file_url = format!(
                "{}/{}",
                self.base_tar_url,
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

        let new_url = Url::parse(&format!("{}/{}", self.base_tar_url, new_path.as_ref()))?;

        Ok(Box::new(TarDir {
            url: new_url,
            store: self.store.clone(),
            path: new_path,
            base_tar_url: self.base_tar_url.clone(),
            writable: self.writable,
        }))
    }

    fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        let new_path = if self.path.as_ref().is_empty() {
            ObjectPath::from(name.trim_start_matches('/'))
        } else {
            self.path.child(name.trim_start_matches('/'))
        };

        let new_url = Url::parse(&format!("{}/{}", self.base_tar_url, new_path.as_ref()))?;

        Ok(Box::new(TarFile::new(
            new_url,
            self.store.clone(),
            new_path,
            self.writable,
        )))
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

        let new_url = Url::parse(&format!("{}/{}", self.base_tar_url, new_path.as_ref()))?;

        Ok(Box::new(TarDir {
            url: new_url,
            store: self.store.clone(),
            path: new_path,
            base_tar_url: self.base_tar_url.clone(),
            writable: self.writable,
        }))
    }

    fn writable_file(&self, name: &str) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError> {
        let new_path = if self.path.as_ref().is_empty() {
            ObjectPath::from(name.trim_start_matches('/'))
        } else {
            self.path.child(name.trim_start_matches('/'))
        };

        let new_url = Url::parse(&format!("{}/{}", self.base_tar_url, new_path.as_ref()))?;

        Ok(Box::new(TarFile::new(
            new_url,
            self.store.clone(),
            new_path,
            self.writable,
        )))
    }

    async fn rename(&self, _from: &str, _to: &str) -> Result<(), BundlebaseError> {
        Err("Tar archives do not support rename".into())
    }
}

// ============================================================================
// TarIOFactory - Factory for creating Tar IO instances
// ============================================================================

/// Factory for Tar IO backends. Handles both local (`tar+file://`) and
/// remote (`tar+s3://`, `tar+gs://`, `tar+azure://`, `tar+az://`) schemes.
pub struct TarIOFactory;

#[async_trait]
impl IOFactory for TarIOFactory {
    fn schemes(&self) -> &[&str] {
        &["tar+file", "tar+s3", "tar+gs", "tar+azure", "tar+az"]
    }

    fn supports_write(&self, url: &Url) -> bool {
        // Only local tar archives support writes
        url.scheme() == "tar+file"
    }

    fn supports_streaming_write(&self) -> bool {
        // Tar format requires knowing file size upfront for the entry header,
        // so we must buffer the entire stream content before writing.
        false
    }

    async fn create_reader(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
        Ok(Box::new(TarFile::from_url(url, &config)?))
    }

    async fn create_lister(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
        Ok(Box::new(TarDir::from_url(url, &config)?))
    }

    async fn create_writable_lister(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Option<Box<dyn IOReadWriteDir>>, BundlebaseError> {
        if !self.supports_write(url) {
            return Ok(None);
        }
        Ok(Some(Box::new(TarDir::from_url(url, &config)?)))
    }

    async fn create_writer(
        &self,
        url: &Url,
        config: Arc<dyn ConfigProvider>,
    ) -> Result<Option<Box<dyn IOReadWriteFile>>, BundlebaseError> {
        if !self.supports_write(url) {
            return Ok(None);
        }
        Ok(Some(Box::new(TarFile::from_url(url, &config)?)))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::test_config;
    use tempfile::NamedTempFile;

    // TarObjectStore tests

    #[tokio::test]
    async fn test_tar_store_head_on_missing_tar_returns_not_found() {
        // When the tar file doesn't exist yet (e.g. fresh BundleBuilder::create
        // into a tar+file:// URL), HEAD on any entry must return NotFound rather
        // than the underlying ENOENT — otherwise BundleBuilder's "is there
        // already a bundle here?" check fails before any file is written.
        let temp_dir = tempfile::tempdir().unwrap();
        let tar_path = temp_dir.path().join("does-not-exist.tar");
        assert!(!tar_path.exists());

        let store = TarObjectStore::new(tar_path).unwrap();
        let err = store
            .head(&ObjectPath::from("anything"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, object_store::Error::NotFound { .. }),
            "expected NotFound, got {:?}",
            err
        );
    }

    #[tokio::test]
    async fn test_tar_store_first_put_creates_tar() {
        // First write into a not-yet-existing tar path should create the file.
        let temp_dir = tempfile::tempdir().unwrap();
        let tar_path = temp_dir.path().join("brand-new.tar");
        let store = TarObjectStore::new(tar_path.clone()).unwrap();

        store
            .put(
                &ObjectPath::from("greeting.txt"),
                PutPayload::from_bytes(Bytes::from_static(b"hi")),
            )
            .await
            .unwrap();

        assert!(tar_path.exists(), "tar file should have been created");
        let result = store.get(&ObjectPath::from("greeting.txt")).await.unwrap();
        assert_eq!(result.bytes().await.unwrap(), Bytes::from_static(b"hi"));
    }

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
        assert_eq!(
            results[0].as_ref().unwrap().location.as_ref(),
            "dir1/file1.txt"
        );
        assert_eq!(
            results[1].as_ref().unwrap().location.as_ref(),
            "dir1/file2.txt"
        );
        assert_eq!(
            results[2].as_ref().unwrap().location.as_ref(),
            "dir2/file3.txt"
        );

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

    #[tokio::test]
    async fn test_tar_store_skips_manifest_in_index() {
        let temp_file = NamedTempFile::new().unwrap();
        let tar_path = temp_file.path().to_path_buf();

        // Build a tar with a manifest entry and a data entry
        {
            let file = File::create(&tar_path).unwrap();
            let mut builder = Builder::new(file);

            let manifest_data = b"[]";
            let mut header = Header::new_gnu();
            header.set_size(manifest_data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            builder
                .append_data(&mut header, TAR_MANIFEST_FILENAME, &manifest_data[..])
                .unwrap();

            let file_data = b"hello";
            let mut header = Header::new_gnu();
            header.set_size(file_data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            builder
                .append_data(&mut header, "data.txt", &file_data[..])
                .unwrap();

            builder.finish().unwrap();
        }

        let store = TarObjectStore::new(tar_path).unwrap();
        let results: Vec<_> = store.list(None).collect::<Vec<_>>().await;

        // Should only see data.txt, not the manifest
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap().location.as_ref(), "data.txt");
    }

    // Tar URL parsing tests

    #[test]
    fn test_parse_tar_url_with_internal_path() {
        let config = test_config();
        let url = Url::parse("tar+file:///home/user/data.tar/subdir/file.parquet").unwrap();
        let (location, internal_path) = parse_tar_url(&url, &config).unwrap();
        assert!(
            matches!(location, TarArchiveLocation::Local(ref p) if *p == PathBuf::from("/home/user/data.tar"))
        );
        assert_eq!(internal_path, "subdir/file.parquet");
    }

    #[test]
    fn test_parse_tar_url_root() {
        let config = test_config();
        let url = Url::parse("tar+file:///data.tar/").unwrap();
        let (location, internal_path) = parse_tar_url(&url, &config).unwrap();
        assert!(
            matches!(location, TarArchiveLocation::Local(ref p) if *p == PathBuf::from("/data.tar"))
        );
        assert_eq!(internal_path, "");
    }

    #[test]
    fn test_parse_tar_url_no_internal_path() {
        let config = test_config();
        let url = Url::parse("tar+file:///archive.tar").unwrap();
        let (location, internal_path) = parse_tar_url(&url, &config).unwrap();
        assert!(
            matches!(location, TarArchiveLocation::Local(ref p) if *p == PathBuf::from("/archive.tar"))
        );
        assert_eq!(internal_path, "");
    }

    #[test]
    fn test_parse_tar_url_wrong_scheme() {
        let config = test_config();
        let url = Url::parse("file:///data.tar").unwrap();
        let result = parse_tar_url(&url, &config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Expected 'tar+<scheme>'"));
    }

    #[test]
    fn test_parse_tar_url_no_tar_extension() {
        let config = test_config();
        let url = Url::parse("tar+file:///data/file.txt").unwrap();
        let result = parse_tar_url(&url, &config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must contain .tar"));
    }

    #[test]
    fn test_parse_tar_url_s3_returns_remote() {
        let config = test_config();
        let url = Url::parse("tar+s3://mybucket/path/data.tar/some/file.csv").unwrap();
        let result = parse_tar_url(&url, &config);
        // This will fail because we don't have real S3 credentials,
        // but we can verify it attempts to create a remote store
        // (the error should be about building the store, not about parsing)
        match result {
            Ok((location, internal_path)) => {
                assert!(matches!(location, TarArchiveLocation::Remote { .. }));
                assert_eq!(internal_path, "some/file.csv");
            }
            Err(e) => {
                // Expected: S3 builder may fail without credentials, that's OK
                let msg = e.to_string();
                assert!(
                    !msg.contains("Expected 'tar+<scheme>'") && !msg.contains("must contain .tar"),
                    "Should not fail on URL parsing, got: {}",
                    msg
                );
            }
        }
    }

    // Offset computation tests

    #[test]
    fn test_pad512() {
        assert_eq!(pad512(0), 0);
        assert_eq!(pad512(1), 512);
        assert_eq!(pad512(511), 512);
        assert_eq!(pad512(512), 512);
        assert_eq!(pad512(513), 1024);
        assert_eq!(pad512(1024), 1024);
    }

    #[test]
    fn test_compute_offsets_empty() {
        let result = compute_offsets(100, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_compute_offsets_single_entry() {
        let entries = vec![("data.txt".to_string(), 100u64)];
        let result = compute_offsets(50, &entries);

        // manifest: header(512) + pad512(50) = 512 + 512 = 1024
        // data.txt: header at 1024, data at 1024+512 = 1536
        let entry = result.get(&ObjectPath::from("data.txt")).unwrap();
        assert_eq!(entry.offset, 1536);
        assert_eq!(entry.size, 100);
    }

    #[test]
    fn test_compute_offsets_multiple_entries() {
        let entries = vec![
            ("file1.txt".to_string(), 100u64),
            ("file2.txt".to_string(), 600u64),
        ];
        let result = compute_offsets(50, &entries);

        // manifest: header(512) + pad512(50) = 512 + 512 = 1024
        // file1: header at 1024, data at 1536, size=100, padded=512, next at 1536+512=2048
        // file2: header at 2048, data at 2560, size=600
        let e1 = result.get(&ObjectPath::from("file1.txt")).unwrap();
        assert_eq!(e1.offset, 1536);
        assert_eq!(e1.size, 100);

        let e2 = result.get(&ObjectPath::from("file2.txt")).unwrap();
        assert_eq!(e2.offset, 2560);
        assert_eq!(e2.size, 600);
    }

    // ReadOnlyTarObjectStore tests

    #[tokio::test]
    async fn test_readonly_tar_store_with_manifest() {
        // Create a tar file with a manifest, then wrap it in InMemory store
        let mut tar_buffer = Vec::new();
        {
            let mut builder = Builder::new(&mut tar_buffer);

            // Write manifest as first entry
            let manifest = serde_json::json!([
                {"name": "hello.txt", "size": 5},
                {"name": "subdir/world.txt", "size": 11},
            ]);
            let manifest_bytes = serde_json::to_vec(&manifest).unwrap();

            let mut header = Header::new_gnu();
            header.set_size(manifest_bytes.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            builder
                .append_data(&mut header, TAR_MANIFEST_FILENAME, &manifest_bytes[..])
                .unwrap();

            // Write hello.txt
            let data = b"hello";
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            builder
                .append_data(&mut header, "hello.txt", &data[..])
                .unwrap();

            // Write subdir/world.txt
            let data = b"hello world";
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            builder
                .append_data(&mut header, "subdir/world.txt", &data[..])
                .unwrap();

            builder.finish().unwrap();
        }

        // Put the tar into an InMemory object store
        let mem_store = object_store::memory::InMemory::new();
        let archive_path = ObjectPath::from("test.tar");
        mem_store
            .put(
                &archive_path,
                PutPayload::from_bytes(Bytes::from(tar_buffer)),
            )
            .await
            .unwrap();

        // Create ReadOnlyTarObjectStore
        let store = ReadOnlyTarObjectStore::new(Arc::new(mem_store), archive_path);

        // Test reading hello.txt
        let result = store.get(&ObjectPath::from("hello.txt")).await.unwrap();
        let bytes = result.bytes().await.unwrap();
        assert_eq!(&bytes[..], b"hello");

        // Test reading subdir/world.txt
        let result = store
            .get(&ObjectPath::from("subdir/world.txt"))
            .await
            .unwrap();
        let bytes = result.bytes().await.unwrap();
        assert_eq!(&bytes[..], b"hello world");

        // Test head
        let meta = store.head(&ObjectPath::from("hello.txt")).await.unwrap();
        assert_eq!(meta.size, 5);

        // Test not found
        let err = store.get(&ObjectPath::from("nope.txt")).await;
        assert!(matches!(err, Err(object_store::Error::NotFound { .. })));

        // Test list
        let all: Vec<_> = store.list(None).collect::<Vec<_>>().await;
        assert_eq!(all.len(), 2);

        // Test writes are rejected
        let put_err = store
            .put(
                &ObjectPath::from("new.txt"),
                PutPayload::from_bytes(Bytes::from("data")),
            )
            .await;
        assert!(matches!(
            put_err,
            Err(object_store::Error::NotSupported { .. })
        ));
    }

    #[tokio::test]
    async fn test_readonly_tar_store_no_manifest_error() {
        // Create a tar without a manifest
        let mut tar_buffer = Vec::new();
        {
            let mut builder = Builder::new(&mut tar_buffer);

            let data = b"hello";
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_cksum();
            builder
                .append_data(&mut header, "hello.txt", &data[..])
                .unwrap();

            builder.finish().unwrap();
        }

        let mem_store = object_store::memory::InMemory::new();
        let archive_path = ObjectPath::from("no_manifest.tar");
        mem_store
            .put(
                &archive_path,
                PutPayload::from_bytes(Bytes::from(tar_buffer)),
            )
            .await
            .unwrap();

        let store = ReadOnlyTarObjectStore::new(Arc::new(mem_store), archive_path);

        // Should fail with a clear error about missing manifest
        let err = store.get(&ObjectPath::from("hello.txt")).await;
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("bundlebase manifest"),
            "Error should mention manifest, got: {}",
            msg
        );
        assert!(
            msg.contains("export_tar()"),
            "Error should mention export_tar(), got: {}",
            msg
        );
    }

    // base_tar_url tests

    #[test]
    fn test_base_tar_url_with_internal_path() {
        let url = Url::parse("tar+file:///path/to/data.tar/subdir/file.txt").unwrap();
        assert_eq!(
            base_tar_url(&url, "subdir/file.txt"),
            "tar+file:///path/to/data.tar"
        );
    }

    #[test]
    fn test_base_tar_url_empty_internal() {
        let url = Url::parse("tar+file:///path/to/data.tar/").unwrap();
        assert_eq!(base_tar_url(&url, ""), "tar+file:///path/to/data.tar");
    }

    #[test]
    fn test_base_tar_url_no_trailing_slash() {
        let url = Url::parse("tar+file:///path/to/data.tar").unwrap();
        assert_eq!(base_tar_url(&url, ""), "tar+file:///path/to/data.tar");
    }

    // TarIOFactory tests

    #[test]
    fn test_tar_factory_schemes() {
        let factory = TarIOFactory;
        let schemes = factory.schemes();
        assert!(schemes.contains(&"tar+file"));
        assert!(schemes.contains(&"tar+s3"));
        assert!(schemes.contains(&"tar+gs"));
        assert!(schemes.contains(&"tar+azure"));
        assert!(schemes.contains(&"tar+az"));
    }

    #[test]
    fn test_tar_factory_supports_write() {
        let factory = TarIOFactory;
        assert!(factory.supports_write(&Url::parse("tar+file:///data.tar/file.txt").unwrap()));
        assert!(!factory.supports_write(&Url::parse("tar+s3://bucket/data.tar/file.txt").unwrap()));
        assert!(!factory.supports_write(&Url::parse("tar+gs://bucket/data.tar/file.txt").unwrap()));
        assert!(!factory
            .supports_write(&Url::parse("tar+azure://container/data.tar/file.txt").unwrap()));
    }
}
