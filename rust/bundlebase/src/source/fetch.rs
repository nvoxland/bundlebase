//! Fetch orchestration for source data discovery and materialization.

use crate::connector::{
    AttachedFileInfo, Connector, DiscoveredLocation, FetchAction, MaterializedData, SourceData,
};
use super::shared_utils::{filename_from_url, record_batch_stream_to_parquet, should_copy};
use super::SyncMode;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::{IOReadFile, IOReadWriteDir, WriteResult};
use crate::progress::ProgressScope;
use crate::{BundleConfig, BundlebaseError};
use bytes::Bytes;
use futures::stream;
use futures::StreamExt;
use log::debug;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Download data and save it to the data directory using content-addressed storage.
///
/// Returns a WriteResult containing the file reference and the computed SHA256 hash.
pub async fn download_to_data_dir(
    data: Bytes,
    filename: &str,
    data_dir: &dyn IOReadWriteDir,
) -> Result<WriteResult, BundlebaseError> {
    // Extract extension from filename (e.g., "file.parquet" -> "parquet")
    let ext = filename.rsplit('.').next().unwrap_or("dat");

    // Create a stream from the bytes
    let data_stream = Box::pin(stream::once(async { Ok::<_, std::io::Error>(data) }));

    data_dir.write_stream(data_stream, ext).await
}

/// Download a file from an IOFile to the data directory.
///
/// Returns a WriteResult containing the file reference and the computed SHA256 hash.
pub async fn download_io_file_to_data_dir(
    file: &ObjectStoreFile,
    data_dir: &dyn IOReadWriteDir,
) -> Result<WriteResult, BundlebaseError> {
    let data = file.read_bytes().await?.ok_or_else(|| {
        BundlebaseError::from(format!("File not found: {}", file.url()))
    })?;
    let filename = filename_from_url(file.url());
    download_to_data_dir(data, &filename, data_dir).await
}

/// Download a file from an HTTP(S) URL to the data directory.
///
/// Returns a WriteResult containing the file reference and the computed SHA256 hash.
///
/// If `format_hint` is provided and the filename from the URL doesn't have a recognized
/// extension, the format hint is appended as the extension.
pub async fn download_http_to_data_dir(
    url: &Url,
    data_dir: &dyn IOReadWriteDir,
    format_hint: Option<&str>,
) -> Result<WriteResult, BundlebaseError> {
    use log::info;

    info!("Downloading {}", url);

    let response = reqwest::get(url.as_str())
        .await
        .map_err(|e| BundlebaseError::from(format!("Failed to download '{}': {}", url, e)))?;

    if !response.status().is_success() {
        return Err(format!(
            "Failed to download '{}': HTTP {}",
            url,
            response.status()
        )
        .into());
    }

    // Log content length if available
    if let Some(len) = response.content_length() {
        info!("Downloading {} ({:.1} MB)", url, len as f64 / 1_048_576.0);
    }

    let data = response
        .bytes()
        .await
        .map_err(|e| BundlebaseError::from(format!("Failed to read '{}': {}", url, e)))?;

    info!("Downloaded {} ({:.1} MB)", url, data.len() as f64 / 1_048_576.0);

    let mut filename = filename_from_url(url);

    // If the filename has no recognized data extension and we have a format hint, append it
    if let Some(fmt) = format_hint {
        let known_extensions = ["csv", "json", "jsonl", "parquet", "tsv", "xml"];
        let has_known_ext = filename
            .rsplit('.')
            .next()
            .map(|ext| known_extensions.contains(&ext.to_lowercase().as_str()))
            .unwrap_or(false);
        if !has_known_ext {
            filename = format!("{}.{}", filename, fmt);
        }
    }

    download_to_data_dir(data, &filename, data_dir).await
}

/// Result of materializing a file, containing the file reference and its hash.
#[derive(Debug)]
pub struct MaterializeResult {
    /// Reference to the file (either copied to data_dir or original location)
    pub file: Box<dyn IOReadFile>,
    /// SHA256 hash of the content (full 64-character hex string)
    pub hash: String,
}

/// Materialize a file from any supported URL scheme to the data directory.
///
/// Handles HTTP(S) via reqwest, other schemes via IOFile.
/// If should_copy is false, returns a file reference to the original URL
/// and computes the hash by streaming the file content.
///
/// Returns both the file reference and its SHA256 hash.
pub async fn materialize_url(
    url: &Url,
    should_copy: bool,
    data_dir: &dyn IOReadWriteDir,
    config: &Arc<BundleConfig>,
    format_hint: Option<&str>,
) -> Result<MaterializeResult, BundlebaseError> {
    if !should_copy {
        let file: Box<dyn IOReadFile> = Box::new(ObjectStoreFile::from_url(url, config.clone())?);
        let hash = file.compute_hash().await?;
        return Ok(MaterializeResult { file, hash });
    }

    if url.scheme() == "http" || url.scheme() == "https" {
        let result = download_http_to_data_dir(url, data_dir, format_hint).await?;
        Ok(MaterializeResult {
            file: result.file,
            hash: result.hash,
        })
    } else {
        let file = ObjectStoreFile::from_url(url, config.clone())?;
        let result = download_io_file_to_data_dir(&file, data_dir).await?;
        Ok(MaterializeResult {
            file: result.file,
            hash: result.hash,
        })
    }
}

/// Internal result from fetching data for a single location.
struct FetchedData {
    /// Where the data was stored (relative path in data_dir or URL)
    attach_location: String,
    /// Source URL for tracking
    source_url: String,
    /// SHA256 hash of the content. None if not yet computed (deferred to attach).
    hash: Option<String>,
}

/// Get data for a discovered location using data() or stable_url().
async fn get_data_for_location(
    func: &dyn Connector,
    location: &DiscoveredLocation,
    args: &HashMap<String, String>,
    config: &Arc<BundleConfig>,
    data_dir: &dyn IOReadWriteDir,
    should_copy: bool,
) -> Result<FetchedData, BundlebaseError> {
    // Try data() first
    if let Some(source_data) = func.data(location, args, config).await? {
        match source_data {
            SourceData::Arrow(batch_stream) => {
                let bytes = record_batch_stream_to_parquet(batch_stream).await?;
                let filename = format!("data.{}", location.format);
                let result = download_to_data_dir(bytes, &filename, data_dir).await?;
                let attach_location = data_dir
                    .relative_path(result.file.as_ref())
                    .unwrap_or_else(|_| result.file.url().to_string());
                return Ok(FetchedData {
                    attach_location,
                    source_url: location.location.clone(),
                    hash: Some(result.hash),
                });
            }
            SourceData::RawBytes(byte_stream) => {
                let ext = location.format.as_str();
                let result = data_dir.write_stream(byte_stream, ext).await?;
                let attach_location = data_dir
                    .relative_path(result.file.as_ref())
                    .unwrap_or_else(|_| result.file.url().to_string());
                return Ok(FetchedData {
                    attach_location,
                    source_url: location.location.clone(),
                    hash: Some(result.hash),
                });
            }
        }
    }

    // Try stable_url()
    if let Some(stable) = func.stable_url(location, args, config).await? {
        if should_copy || location.must_copy {
            // Download the file into data_dir
            let result = materialize_url(&stable, true, data_dir, config, Some(&location.format)).await?;
            let attach_location = data_dir
                .relative_path(result.file.as_ref())
                .unwrap_or_else(|_| result.file.url().to_string());
            Ok(FetchedData {
                attach_location,
                source_url: stable.to_string(),
                hash: Some(result.hash),
            })
        } else {
            // Reference URL directly, hash will be computed during attach
            Ok(FetchedData {
                attach_location: stable.to_string(),
                source_url: stable.to_string(),
                hash: None,
            })
        }
    } else {
        Err(format!(
            "Connector returned neither data nor stable_url for location '{}'",
            location.location
        )
        .into())
    }
}

/// Orchestrate a fetch operation with sync mode support.
///
/// This is the unified orchestration function that replaces `process_sync_mode`
/// and the per-function `fetch_with_mode` overrides.
///
/// Logic:
/// 1. Call `func.discover()` to get all matching locations with versions
/// 2. For each discovered location:
///    - If in attached_files: skip for Add mode; for Update/Sync compare versions
///    - If not in attached_files: get data and emit Add
/// 3. For Sync mode: emit Remove for attached locations not in discovered set
/// 4. "Get data" tries `func.data()` first, then `func.stable_url()`
pub async fn orchestrate_fetch(
    func: &dyn Connector,
    args: &HashMap<String, String>,
    mode: SyncMode,
    should_copy: bool,
    data_dir: &dyn IOReadWriteDir,
    attached_files: &HashMap<String, AttachedFileInfo>,
    config: &Arc<BundleConfig>,
) -> Result<Vec<FetchAction>, BundlebaseError> {
    let attached_locations: HashSet<String> = attached_files.keys().cloned().collect();
    let discovered = func.discover(args, &attached_locations, config).await?;

    // Build set of discovered locations for Remove detection
    let discovered_locations: HashSet<String> =
        discovered.iter().map(|d| d.location.clone()).collect();

    let progress = ProgressScope::new(
        &format!("Processing {} discovered files", discovered.len()),
        Some(discovered.len() as u64),
    );

    let mut actions = Vec::new();

    for (idx, location) in discovered.iter().enumerate() {
        progress.update(idx as u64, Some(&location.location));

        if let Some(attached_info) = attached_files.get(&location.location) {
            // Already attached — check for changes in Update/Sync mode
            if mode == SyncMode::Update || mode == SyncMode::Sync {
                if location.version != attached_info.version {
                    debug!(
                        "File {} changed: version {} -> {}",
                        location.location, attached_info.version, location.version
                    );
                    let data = get_data_for_location(
                        func,
                        location,
                        args,
                        config,
                        data_dir,
                        should_copy,
                    )
                    .await?;
                    actions.push(FetchAction::Replace {
                        old_source_location: location.location.clone(),
                        data: MaterializedData {
                            attach_location: data.attach_location,
                            source_location: location.location.clone(),
                            source_url: data.source_url,
                            hash: data.hash,
                            version: location.version.clone(),
                        },
                    });
                }
            }
            // For Add mode, skip files that are already attached
        } else {
            // New file — add it
            let data =
                get_data_for_location(func, location, args, config, data_dir, should_copy).await?;
            actions.push(FetchAction::Add(MaterializedData {
                attach_location: data.attach_location,
                source_location: location.location.clone(),
                source_url: data.source_url,
                hash: data.hash,
                version: location.version.clone(),
            }));
        }
    }

    // For Sync mode: find removed files
    if mode == SyncMode::Sync {
        for source_location in attached_files.keys() {
            if !discovered_locations.contains(source_location) {
                debug!("File {} no longer exists at remote", source_location);
                actions.push(FetchAction::Remove {
                    source_location: source_location.clone(),
                });
            }
        }
    }

    Ok(actions)
}
