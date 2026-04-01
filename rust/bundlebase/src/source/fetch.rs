//! Fetch orchestration for source data discovery and materialization.

use crate::connector::{
    AttachedFileInfo, Connector, SaveAs, DiscoveredLocation, FetchAction, MaterializedData,
    ResolvedSaveAs, SourceData,
};
use bundlebase_common::source_utils::{convert_to_parquet, detect_format_from_bytes, filename_from_url, http_status_error, record_batch_stream_to_parquet};
use super::SyncMode;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::{IOReadFile, IOReadWriteDir, WriteResult};
use crate::progress::ProgressScope;
use crate::{BundlebaseError, ConfigProvider};
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
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(http_status_error(url, status, Some(&body)).into());
    }

    // Check Content-Type for error responses disguised as 200 OK.
    // If the server returns text/html when we expect data, it's likely an error page.
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_lowercase());
    if let Some(ref ct) = content_type {
        let mime = ct.split(';').next().unwrap_or("").trim();
        if mime == "text/html" || mime == "application/xhtml+xml" {
            return Err(BundlebaseError::from(format!(
                "URL '{}' returned HTML content (Content-Type: {}). \
                 This is likely an error page or login redirect, not a data file. \
                 Verify the URL points directly to downloadable data.",
                url, ct
            )));
        }
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

    // Resolve "auto" format hint by inspecting content
    let resolved_hint = match format_hint {
        Some("auto") => {
            match detect_format_from_bytes(&data) {
                Some(fmt) => Some(fmt),
                None => {
                    return Err(BundlebaseError::from(format!(
                        "Could not detect format of data from '{}'. Specify format explicitly with WITH (format='csv')",
                        url
                    )));
                }
            }
        }
        other => other,
    };

    let mut filename = filename_from_url(url);

    // If the filename has no recognized data extension and we have a format hint, append it
    if let Some(fmt) = resolved_hint {
        let known_extensions = ["csv", "json", "jsonl", "parquet", "tsv", "xml", "xlsx", "xls", "ods"];
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

/// Collect a byte stream into a single Bytes buffer.
async fn collect_byte_stream(
    mut stream: futures::stream::BoxStream<'static, Result<Bytes, std::io::Error>>,
) -> Result<Bytes, BundlebaseError> {
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| BundlebaseError::from(format!("Failed to read byte stream: {}", e)))?;
        buffer.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(buffer))
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
    config: &Arc<dyn ConfigProvider>,
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
    config: &Arc<dyn ConfigProvider>,
    data_dir: &dyn IOReadWriteDir,
    save_as: &SaveAs,
) -> Result<FetchedData, BundlebaseError> {
    // Resolve copy strategy based on source's save_as, format, and must_copy
    let strategy = save_as.resolve(&location.format, location.must_copy)?;

    // Try data() first
    if let Some(source_data) = func.data(location, args, config).await? {
        return save_source_data(source_data, &strategy, &location.format, &location.location, data_dir).await;
    }

    // Try stable_url()
    if let Some(stable) = func.stable_url(location, args, config).await? {
        match &strategy {
            ResolvedSaveAs::Ref => {
                // Reference URL directly — no download
                return Ok(FetchedData {
                    attach_location: stable.to_string(),
                    source_url: stable.to_string(),
                    hash: None,
                });
            }
            _ => {
                return save_from_url(&stable, &strategy, &location.format, data_dir, config).await;
            }
        }
    }

    Err(format!(
        "Connector returned neither data nor stable_url for location '{}'",
        location.location
    )
    .into())
}

/// Save SourceData (Arrow or RawBytes) to the data directory using the resolved save strategy.
async fn save_source_data(
    source_data: SourceData,
    strategy: &ResolvedSaveAs,
    format: &crate::connector::SourceFormat,
    source_url: &str,
    data_dir: &dyn IOReadWriteDir,
) -> Result<FetchedData, BundlebaseError> {
    match (source_data, strategy) {
        // Arrow data always serializes to Parquet regardless of strategy.
        // Arrow batches can't be "copied" or "ref'd" as files.
        (SourceData::Arrow(batch_stream), _) => {
            let bytes = record_batch_stream_to_parquet(batch_stream).await?;
            let result = download_to_data_dir(bytes, "data.parquet", data_dir).await?;
            Ok(make_fetched_data(data_dir, &result, source_url))
        }
        (SourceData::RawBytes(byte_stream), ResolvedSaveAs::Copy) => {
            let result = data_dir.write_stream(byte_stream, format.extension()).await?;
            Ok(make_fetched_data(data_dir, &result, source_url))
        }
        (SourceData::RawBytes(byte_stream), ResolvedSaveAs::Parquet) => {
            let bytes = collect_byte_stream(byte_stream).await?;
            let parquet_bytes = convert_to_parquet(&bytes, format)?;
            let result = download_to_data_dir(parquet_bytes, "data.parquet", data_dir).await?;
            Ok(make_fetched_data(data_dir, &result, source_url))
        }
        (SourceData::RawBytes(_), ResolvedSaveAs::Ref) => {
            // Ref with data() shouldn't happen — data() returns bytes to store,
            // not a URL reference. This would be a connector bug.
            Err("Connector returned data bytes but save_as resolved to 'ref'. \
                 This is unexpected — connectors that support ref should use stable_url().".into())
        }
    }
}

/// Download from a URL and save using the resolved save strategy.
async fn save_from_url(
    url: &Url,
    strategy: &ResolvedSaveAs,
    format: &crate::connector::SourceFormat,
    data_dir: &dyn IOReadWriteDir,
    config: &Arc<dyn ConfigProvider>,
) -> Result<FetchedData, BundlebaseError> {
    match strategy {
        ResolvedSaveAs::Copy => {
            let result = materialize_url(url, true, data_dir, config, Some(format.extension())).await?;
            let attach_location = data_dir
                .relative_path(result.file.as_ref())
                .unwrap_or_else(|_| result.file.url().to_string());
            Ok(FetchedData {
                attach_location,
                source_url: url.to_string(),
                hash: Some(result.hash),
            })
        }
        ResolvedSaveAs::Parquet => {
            // Download raw bytes, convert to Parquet, then write
            let response = reqwest::get(url.as_str())
                .await
                .map_err(|e| BundlebaseError::from(format!("Failed to download '{}': {}", url, e)))?;
            if !response.status().is_success() {
                return Err(http_status_error(url, response.status(), None).into());
            }
            let raw_bytes = response.bytes().await
                .map_err(|e| BundlebaseError::from(format!("Failed to read '{}': {}", url, e)))?;
            let parquet_bytes = convert_to_parquet(&raw_bytes, format)?;
            let result = download_to_data_dir(parquet_bytes, "data.parquet", data_dir).await?;
            Ok(make_fetched_data(data_dir, &result, url.as_str()))
        }
        ResolvedSaveAs::Ref => {
            // Ref is handled in get_data_for_location before calling save_from_url
            unreachable!("Ref strategy should not reach save_from_url")
        }
    }
}

/// Build FetchedData from a WriteResult.
fn make_fetched_data(data_dir: &dyn IOReadWriteDir, result: &WriteResult, source_url: &str) -> FetchedData {
    let attach_location = data_dir
        .relative_path(result.file.as_ref())
        .unwrap_or_else(|_| result.file.url().to_string());
    FetchedData {
        attach_location,
        source_url: source_url.to_string(),
        hash: Some(result.hash.clone()),
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
    save_as: &SaveAs,
    data_dir: &dyn IOReadWriteDir,
    attached_files: &HashMap<String, AttachedFileInfo>,
    config: &Arc<dyn ConfigProvider>,
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
                        &save_as,
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
                get_data_for_location(func, location, args, config, data_dir, &save_as).await?;
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
