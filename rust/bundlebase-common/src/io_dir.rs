//! Directory IO traits for reading and writing directories.

use crate::content_address::ContentAddress;
use crate::BundlebaseError;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use rand::RngExt;
use std::fmt::Debug;
use url::Url;
use xxhash_rust::xxh3::Xxh3;

use crate::file_info::FileInfo;
use crate::io_file::{IOReadFile, IOReadWriteFile};

/// Result of writing a stream to a content-addressed file.
/// Contains both the file reference and the computed SHA256 hash.
#[derive(Debug)]
pub struct WriteResult {
    /// Reference to the written file
    pub file: Box<dyn IOReadFile>,
    /// xxHash (xxh3-128) of the content (32-character hex string)
    pub hash: String,
}

/// Read-only directory operations.
/// Implemented by all storage backends - both read-only sources (FTP) and read-write stores.
#[async_trait]
pub trait IOReadDir: Send + Sync + Debug {
    /// Returns the URL this directory represents.
    fn url(&self) -> &Url;

    /// List all files in this directory.
    async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError>;

    /// Get a subdirectory reference.
    /// The subdirectory is not validated to exist.
    fn subdir(&self, name: &str) -> Result<Box<dyn IOReadDir>, BundlebaseError>;

    /// Get a file reference within this directory.
    /// The file is not validated to exist.
    fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError>;

    /// Get the relative path of a file within this directory.
    ///
    /// Returns the path of the file relative to this directory's URL.
    /// Returns an error if the file is not within this directory.
    ///
    /// # Example
    /// If the directory URL is `file:///data/` and the file URL is
    /// `file:///data/ab/cdef.parquet`, returns `"ab/cdef.parquet"`.
    fn relative_path(&self, file: &dyn IOReadFile) -> Result<String, BundlebaseError> {
        let dir_url = self.url();
        let file_url = file.url();

        // Check that schemes and hosts match
        if dir_url.scheme() != file_url.scheme() {
            return Err(format!(
                "File scheme '{}' does not match directory scheme '{}'",
                file_url.scheme(),
                dir_url.scheme()
            )
            .into());
        }

        if dir_url.host() != file_url.host() {
            return Err(format!(
                "File host '{:?}' does not match directory host '{:?}'",
                file_url.host(),
                dir_url.host()
            )
            .into());
        }

        // Normalize directory path to ensure it ends with /
        let dir_path = dir_url.path();
        let dir_path_normalized = if dir_path.ends_with('/') {
            dir_path.to_string()
        } else {
            format!("{}/", dir_path)
        };

        let file_path = file_url.path();

        // Check that the file path starts with the directory path
        if let Some(relative) = file_path.strip_prefix(&dir_path_normalized) {
            Ok(relative.to_string())
        } else {
            Err(format!("File '{}' is not within directory '{}'", file_url, dir_url).into())
        }
    }
}

/// Write operations for directories that support modification.
/// Not implemented by read-only backends (FTP, SFTP when used as sources).
#[async_trait]
pub trait IOReadWriteDir: IOReadDir {
    /// Check if this directory uses local filesystem storage.
    /// Used to determine optimal temp directory location for index building.
    ///
    /// Returns `true` for local storage backends (file://, tar+file://, memory://)
    /// and `false` for remote storage backends (s3://, gs://, azure://).
    fn is_local_storage(&self) -> bool {
        matches!(self.url().scheme(), "file" | "tar+file" | "memory")
    }

    /// Get a writable subdirectory reference.
    /// The subdirectory is not validated to exist.
    fn writable_subdir(&self, name: &str) -> Result<Box<dyn IOReadWriteDir>, BundlebaseError>;

    /// Get a writable file reference within this directory.
    fn writable_file(&self, name: &str) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError>;

    /// Rename a file within this directory.
    async fn rename(&self, from: &str, to: &str) -> Result<(), BundlebaseError>;

    /// Write data stream to a new file named by its content hash.
    ///
    /// The stream is hashed (xxh3-128) chunk-by-chunk as it is written to a
    /// temporary file in a `temp/` subdirectory — the content is never fully
    /// buffered in memory (ADR-003). The temp file is then renamed to the
    /// content-addressed location (streaming-copied on backends without
    /// rename). If a file with that hash already exists, the temp file is
    /// deleted and the existing file is returned (deduplication).
    ///
    /// Files are organized by hash prefix: the first 2 hex characters become
    /// a subdirectory, and the remaining hash characters become the filename.
    ///
    /// Returns a `WriteResult` containing the file reference and the computed hash.
    async fn write_stream(
        &self,
        source: BoxStream<'static, Result<Bytes, std::io::Error>>,
        address: &ContentAddress,
    ) -> Result<WriteResult, BundlebaseError> {
        use futures::StreamExt;

        // Create temp subdir and write temp file there
        let temp_dir = self.writable_subdir("temp")?;
        let temp_name = format!("tohash_{:016x}", rand::rng().random::<u64>());
        let temp_file = temp_dir.writable_file(&temp_name)?;

        // Hash each chunk as it passes through to the backend's streaming
        // write, so the content is never accumulated in memory.
        let hasher = std::sync::Arc::new(std::sync::Mutex::new(Xxh3::new()));
        let hashing_source = {
            let hasher = std::sync::Arc::clone(&hasher);
            source.map(move |chunk_result| {
                if let Ok(ref chunk) = chunk_result {
                    if let Ok(mut hasher) = hasher.lock() {
                        hasher.update(chunk);
                    }
                }
                chunk_result
            })
        };
        temp_file.write_stream(Box::pin(hashing_source)).await?;

        // Compute hash - first 2 chars = subdir, remaining chars = filename
        let hash = format!(
            "{:032x}",
            hasher
                .lock()
                .map_err(|_| BundlebaseError::from("Hasher lock poisoned"))?
                .digest128()
        );
        let subdir_name = &hash[..2];
        let file_name = format!("{}.{}", &hash[2..16], address.extension());

        // Get or create the hash-prefix subdirectory
        let final_dir = self.writable_subdir(subdir_name)?;

        // Check for duplicate - delete temp or move to final location
        if final_dir.file(&file_name)?.exists().await? {
            // Best-effort delete - some backends (like tar) don't support deletion
            let _ = temp_file.delete().await;
        } else {
            // Move via rename — atomic on local filesystems, a server-side
            // copy on object stores. Backends without rename (tar) fall back
            // to a streaming copy; either way the content never lands in
            // memory as a whole.
            let rename_result = self
                .rename(
                    &format!("temp/{}", temp_name),
                    &format!("{}/{}", subdir_name, file_name),
                )
                .await;
            if let Err(rename_err) = &rename_result {
                // Expected for backends without rename (tar); anything else
                // is a transient failure worth surfacing before the fallback
                // silently re-downloads and re-uploads the whole object.
                log::debug!(
                    "rename to content-addressed location failed ({}); falling back to streaming copy",
                    rename_err
                );
            }
            if rename_result.is_err() {
                let final_file = final_dir.writable_file(&file_name)?;
                let temp_stream = temp_file
                    .read_stream()
                    .await?
                    .ok_or("Temp file missing after write")?;
                let io_stream =
                    temp_stream.map(|r| r.map_err(|e| std::io::Error::other(e.to_string())));
                final_file.write_stream(Box::pin(io_stream)).await?;
                // Best-effort delete - some backends (like tar) don't support deletion
                let _ = temp_file.delete().await;
            }
        }

        let file = final_dir.file(&file_name)?;
        Ok(WriteResult { file, hash })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// In-memory recording backend: stores file contents and records which
    /// file/dir methods the default `write_stream` exercises, so the tests can
    /// pin the streaming behavior (no whole-file buffering paths).
    #[derive(Debug, Default)]
    struct MockState {
        files: HashMap<String, Vec<u8>>,
        calls: Vec<String>,
        fail_rename: bool,
    }

    #[derive(Debug, Clone)]
    struct MockDir {
        url: Url,
        prefix: String,
        state: Arc<Mutex<MockState>>,
    }

    impl MockDir {
        fn root(fail_rename: bool) -> Self {
            MockDir {
                url: Url::parse("mock:///").expect("static url"),
                prefix: String::new(),
                state: Arc::new(Mutex::new(MockState {
                    fail_rename,
                    ..Default::default()
                })),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.state.lock().expect("mock lock").calls.clone()
        }

        fn file_content(&self, path: &str) -> Option<Vec<u8>> {
            self.state.lock().expect("mock lock").files.get(path).cloned()
        }

        fn file_paths(&self) -> Vec<String> {
            let mut paths: Vec<String> = self
                .state
                .lock()
                .expect("mock lock")
                .files
                .keys()
                .cloned()
                .collect();
            paths.sort();
            paths
        }
    }

    #[derive(Debug)]
    struct MockFile {
        url: Url,
        path: String,
        state: Arc<Mutex<MockState>>,
    }

    #[async_trait]
    impl IOReadFile for MockFile {
        fn url(&self) -> &Url {
            &self.url
        }

        async fn exists(&self) -> Result<bool, BundlebaseError> {
            Ok(self
                .state
                .lock()
                .expect("mock lock")
                .files
                .contains_key(&self.path))
        }

        async fn open_stream(
            &self,
        ) -> Result<Option<BoxStream<'static, Result<Bytes, BundlebaseError>>>, BundlebaseError>
        {
            let content = {
                let mut state = self.state.lock().expect("mock lock");
                state.calls.push(format!("open_stream:{}", self.path));
                state.files.get(&self.path).cloned()
            };
            Ok(content.map(|bytes| {
                Box::pin(futures::stream::once(async move {
                    Ok(Bytes::from(bytes))
                })) as BoxStream<'static, Result<Bytes, BundlebaseError>>
            }))
        }

        async fn metadata(&self) -> Result<Option<FileInfo>, BundlebaseError> {
            Ok(None)
        }

        async fn version(&self) -> Result<String, BundlebaseError> {
            Ok("v1".to_string())
        }
    }

    #[async_trait]
    impl IOReadWriteFile for MockFile {
        async fn write(&self, data: Bytes) -> Result<(), BundlebaseError> {
            let mut state = self.state.lock().expect("mock lock");
            state.calls.push(format!("write:{}", self.path));
            state.files.insert(self.path.clone(), data.to_vec());
            Ok(())
        }

        async fn write_stream(
            &self,
            mut source: BoxStream<'static, Result<Bytes, std::io::Error>>,
        ) -> Result<(), BundlebaseError> {
            self.state
                .lock()
                .expect("mock lock")
                .calls
                .push(format!("write_stream:{}", self.path));
            let mut content = Vec::new();
            while let Some(chunk) = source.next().await {
                let chunk = chunk.map_err(|e| BundlebaseError::from(e.to_string()))?;
                content.extend_from_slice(&chunk);
            }
            self.state
                .lock()
                .expect("mock lock")
                .files
                .insert(self.path.clone(), content);
            Ok(())
        }

        async fn delete(&self) -> Result<(), BundlebaseError> {
            let mut state = self.state.lock().expect("mock lock");
            state.calls.push(format!("delete:{}", self.path));
            state.files.remove(&self.path);
            Ok(())
        }
    }

    impl MockDir {
        fn make_file(&self, name: &str) -> MockFile {
            let path = format!("{}{}", self.prefix, name);
            MockFile {
                url: Url::parse(&format!("mock:///{}", path)).expect("static url"),
                path,
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait]
    impl IOReadDir for MockDir {
        fn url(&self) -> &Url {
            &self.url
        }

        async fn list_files(&self) -> Result<Vec<FileInfo>, BundlebaseError> {
            Ok(vec![])
        }

        fn subdir(&self, name: &str) -> Result<Box<dyn IOReadDir>, BundlebaseError> {
            Ok(Box::new(MockDir {
                url: self.url.clone(),
                prefix: format!("{}{}/", self.prefix, name),
                state: Arc::clone(&self.state),
            }))
        }

        fn file(&self, name: &str) -> Result<Box<dyn IOReadFile>, BundlebaseError> {
            Ok(Box::new(self.make_file(name)))
        }
    }

    #[async_trait]
    impl IOReadWriteDir for MockDir {
        fn writable_subdir(&self, name: &str) -> Result<Box<dyn IOReadWriteDir>, BundlebaseError> {
            Ok(Box::new(MockDir {
                url: self.url.clone(),
                prefix: format!("{}{}/", self.prefix, name),
                state: Arc::clone(&self.state),
            }))
        }

        fn writable_file(&self, name: &str) -> Result<Box<dyn IOReadWriteFile>, BundlebaseError> {
            Ok(Box::new(self.make_file(name)))
        }

        async fn rename(&self, from: &str, to: &str) -> Result<(), BundlebaseError> {
            let mut state = self.state.lock().expect("mock lock");
            if state.fail_rename {
                return Err("rename unsupported".into());
            }
            state.calls.push(format!("rename:{}->{}", from, to));
            let from_path = format!("{}{}", self.prefix, from);
            let to_path = format!("{}{}", self.prefix, to);
            let content = state
                .files
                .remove(&from_path)
                .ok_or("rename source missing")?;
            state.files.insert(to_path, content);
            Ok(())
        }
    }

    fn chunk_stream(chunks: Vec<&'static [u8]>) -> BoxStream<'static, Result<Bytes, std::io::Error>> {
        Box::pin(futures::stream::iter(
            chunks.into_iter().map(|c| Ok(Bytes::from_static(c))),
        ))
    }

    fn parquet_address() -> ContentAddress {
        ContentAddress::new(
            crate::content_address::ContentCategory::Block,
            crate::content_address::ContentFormat::Parquet,
        )
    }

    fn expected_hash(content: &[u8]) -> String {
        let mut hasher = Xxh3::new();
        hasher.update(content);
        format!("{:032x}", hasher.digest128())
    }

    /// The stream must reach the backend via the file-level streaming write —
    /// never accumulated and flushed through the whole-Bytes `write` (bb-xn0.1).
    #[tokio::test]
    async fn test_write_stream_streams_to_backend() {
        let dir = MockDir::root(false);
        let result = dir
            .write_stream(chunk_stream(vec![b"hello ", b"world"]), &parquet_address())
            .await
            .expect("write_stream");

        assert_eq!(result.hash, expected_hash(b"hello world"));
        let calls = dir.calls();
        assert!(
            calls.iter().any(|c| c.starts_with("write_stream:temp/")),
            "temp file must be written via streaming, calls: {calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("write:")),
            "whole-buffer write() must not be used, calls: {calls:?}"
        );
    }

    /// Moving temp -> final must use rename, not read the whole temp file
    /// back into memory (bb-xn0.1).
    #[tokio::test]
    async fn test_write_stream_renames_instead_of_rereading() {
        let dir = MockDir::root(false);
        let result = dir
            .write_stream(chunk_stream(vec![b"content"]), &parquet_address())
            .await
            .expect("write_stream");

        let calls = dir.calls();
        assert!(
            calls.iter().any(|c| c.starts_with("rename:temp/")),
            "temp file must be moved via rename, calls: {calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("open_stream:temp/")),
            "temp file must not be read back, calls: {calls:?}"
        );

        // Final layout: <2-char prefix>/<hash[2..16]>.<ext> with the content
        let hash = expected_hash(b"content");
        let final_path = format!("{}/{}.block.parquet", &hash[..2], &hash[2..16]);
        assert_eq!(dir.file_content(&final_path).as_deref(), Some(&b"content"[..]));
        assert_eq!(dir.file_paths(), vec![final_path.clone()]);
        assert!(result.file.url().as_str().ends_with(&final_path));
    }

    /// Backends without rename (tar) fall back to a streaming copy — still no
    /// whole-file buffering, and the temp file is cleaned up.
    #[tokio::test]
    async fn test_write_stream_falls_back_to_streaming_copy_without_rename() {
        let dir = MockDir::root(true);
        dir.write_stream(chunk_stream(vec![b"fallback"]), &parquet_address())
            .await
            .expect("write_stream");

        let calls = dir.calls();
        assert!(
            calls.iter().any(|c| c.starts_with("open_stream:temp/")),
            "fallback must stream the temp file, calls: {calls:?}"
        );
        assert_eq!(
            calls
                .iter()
                .filter(|c| c.starts_with("write_stream:"))
                .count(),
            2,
            "fallback must use streaming writes for temp and final, calls: {calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("write:")),
            "whole-buffer write() must not be used, calls: {calls:?}"
        );

        let hash = expected_hash(b"fallback");
        let final_path = format!("{}/{}.block.parquet", &hash[..2], &hash[2..16]);
        assert_eq!(dir.file_paths(), vec![final_path]);
    }

    /// Deduplication: when the content-addressed file already exists, the temp
    /// file is deleted and the existing file returned.
    #[tokio::test]
    async fn test_write_stream_dedupes_existing_content() {
        let dir = MockDir::root(false);
        dir.write_stream(chunk_stream(vec![b"dup"]), &parquet_address())
            .await
            .expect("first write");
        let result = dir
            .write_stream(chunk_stream(vec![b"dup"]), &parquet_address())
            .await
            .expect("second write");

        assert_eq!(result.hash, expected_hash(b"dup"));
        let hash = expected_hash(b"dup");
        let final_path = format!("{}/{}.block.parquet", &hash[..2], &hash[2..16]);
        // Only the deduped file remains — no temp leftovers
        assert_eq!(dir.file_paths(), vec![final_path]);
    }
}
