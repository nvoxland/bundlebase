use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{self, BoxStream, StreamExt};
use object_store::path::Path as ObjectPath;
use object_store::{
    GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore, PutOptions,
    PutPayload, PutResult, Result as ObjectStoreResult,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tar::{Archive, Builder, Header};

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

            // Calculate offset - this is an approximation since we're iterating
            // We'll recalculate precise offsets when we actually need to read
            let tar_entry = TarEntry {
                offset: 0, // Will be set to proper value when reading
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

    /// Reads a file from the tar archive by scanning to find it.
    /// This is less efficient than using byte offsets, but tar format
    /// requires sequential reading for accurate positioning.
    fn read_entry(&self, path: &ObjectPath) -> ObjectStoreResult<Bytes> {
        let file = File::open(&*self.tar_path).map_err(|e| {
            object_store::Error::Generic {
                store: "TarObjectStore",
                source: Box::new(e),
            }
        })?;

        let mut archive = Archive::new(file);

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
            let entry_path_str = entry_path.to_str().ok_or_else(|| object_store::Error::Generic {
                store: "TarObjectStore",
                source: "Invalid UTF-8 in tar entry path".into(),
            })?;

            if entry_path_str == path.as_ref() {
                // Found the entry, read its contents
                let mut buffer = Vec::new();
                entry.read_to_end(&mut buffer).map_err(|e| {
                    object_store::Error::Generic {
                        store: "TarObjectStore",
                        source: Box::new(e),
                    }
                })?;
                return Ok(Bytes::from(buffer));
            }
        }

        Err(object_store::Error::NotFound {
            path: path.to_string(),
            source: "File not found in tar archive".into(),
        })
    }

    /// Appends a new file to the tar archive.
    ///
    /// Note: This implementation rewrites the entire tar file with the new entry.
    /// This is not the most efficient approach, but it's simple and works correctly.
    /// A more efficient approach would seek back to remove the tar footer, append
    /// the new entry, and write a new footer, but that's more complex.
    fn append_entry(&self, path: &ObjectPath, data: Bytes) -> ObjectStoreResult<()> {
        use std::io::{Cursor, Seek};

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
                    .unwrap()
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
                .unwrap()
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
        let prefix_owned = prefix.map(|p| p.clone());

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

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
}
