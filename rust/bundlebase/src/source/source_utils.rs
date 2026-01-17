//! Shared utilities for source functions.
//!
//! Provides common functionality used by multiple source function implementations.

use super::source_function::{
    AttachedFileInfo, DiscoveredLocation, MaterializedData, FetchAction, SyncMode,
};
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::{IOReadFile, IOReadWriteDir, WriteResult};
use futures::stream;
use crate::{BundleConfig, BundlebaseError};
use bytes::Bytes;
use futures::future::BoxFuture;
use glob::Pattern;
use log::debug;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use url::Url;

/// Parse glob patterns from a comma-separated string.
///
/// Returns compiled patterns ready for matching.
pub fn parse_patterns(patterns_str: &str) -> Result<Vec<Pattern>, BundlebaseError> {
    patterns_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|p| {
            Pattern::new(p).map_err(|e| {
                BundlebaseError::from(format!("Invalid glob pattern '{}': {}", p, e))
            })
        })
        .collect()
}

/// Get patterns from args, returning compiled patterns.
///
/// Uses the "patterns" arg if present, otherwise defaults to "**/*".
pub fn get_patterns(args: &HashMap<String, String>) -> Result<Vec<Pattern>, BundlebaseError> {
    let patterns_str = args
        .get("patterns")
        .map(|s| s.as_str())
        .unwrap_or("**/*");
    parse_patterns(patterns_str)
}

/// Check if a URL matches any of the compiled patterns.
///
/// Matches against both the filename and the full path portion of the URL.
pub fn matches_patterns(url: &Url, patterns: &[Pattern]) -> bool {
    let path = url.path();
    let filename = path.rsplit('/').next().unwrap_or(path);
    patterns.iter().any(|p| p.matches(filename) || p.matches(path.trim_start_matches('/')))
}

/// Check if should_copy is enabled from args (default: true).
pub fn should_copy(args: &HashMap<String, String>) -> bool {
    args.get("copy").map(|s| s != "false").unwrap_or(true)
}

/// Validate the "copy" argument if present.
pub fn validate_copy_arg(
    function_name: &str,
    args: &HashMap<String, String>,
) -> Result<(), BundlebaseError> {
    if let Some(copy_val) = args.get("copy") {
        if copy_val != "true" && copy_val != "false" {
            return Err(format!(
                "Function '{}': 'copy' argument must be 'true' or 'false', got '{}'",
                function_name, copy_val
            )
            .into());
        }
    }
    Ok(())
}

/// Get a required argument from args, returning an error if missing.
pub fn require_arg<'a>(
    args: &'a HashMap<String, String>,
    name: &str,
    function_name: &str,
) -> Result<&'a str, BundlebaseError> {
    args.get(name).map(|s| s.as_str()).ok_or_else(|| {
        BundlebaseError::from(format!(
            "Function '{}' requires a '{}' argument",
            function_name, name
        ))
    })
}

/// Parse and validate a URL from args.
pub fn require_url(
    args: &HashMap<String, String>,
    function_name: &str,
) -> Result<Url, BundlebaseError> {
    let url_str = require_arg(args, "url", function_name)?;
    Url::parse(url_str).map_err(|e| {
        BundlebaseError::from(format!("Invalid URL '{}': {}", url_str, e))
    })
}

/// Extract a filename from a URL path.
///
/// Returns the last path segment, or "data" if none found.
pub fn filename_from_url(url: &Url) -> String {
    url.path_segments()
        .and_then(|s| s.last())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "data".to_string())
}

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
pub async fn download_http_to_data_dir(
    url: &Url,
    data_dir: &dyn IOReadWriteDir,
) -> Result<WriteResult, BundlebaseError> {
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

    let data = response
        .bytes()
        .await
        .map_err(|e| BundlebaseError::from(format!("Failed to read '{}': {}", url, e)))?;

    let filename = filename_from_url(url);
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
) -> Result<MaterializeResult, BundlebaseError> {
    if !should_copy {
        // For non-copied files, compute the hash by streaming
        let file: Box<dyn IOReadFile> = Box::new(ObjectStoreFile::from_url(url, config.clone())?);
        let hash = file.compute_hash().await?;
        return Ok(MaterializeResult { file, hash });
    }

    if url.scheme() == "http" || url.scheme() == "https" {
        let result = download_http_to_data_dir(url, data_dir).await?;
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

/// Read version from an HTTP(S) URL using ETag or Last-Modified header.
///
/// Sends a HEAD request and extracts version information from headers.
/// Uses ETag if available, otherwise Last-Modified, otherwise falls back to status code.
pub async fn read_http_version(url: &Url) -> Result<String, BundlebaseError> {
    let response = reqwest::Client::new()
        .head(url.as_str())
        .send()
        .await
        .map_err(|e| BundlebaseError::from(format!("Failed to HEAD '{}': {}", url, e)))?;

    // Use ETag if available, otherwise Last-Modified, otherwise status
    if let Some(etag) = response.headers().get("etag") {
        return Ok(etag.to_str().unwrap_or("unknown").to_string());
    }
    if let Some(lm) = response.headers().get("last-modified") {
        return Ok(lm.to_str().unwrap_or("unknown").to_string());
    }
    Ok(format!("status-{}", response.status().as_u16()))
}

/// Process sync mode logic for discovered locations.
///
/// This is the shared implementation used by both remote_dir and web_scrape.
/// It handles the core sync logic:
/// - For new files: generate Add action
/// - For existing files in Update/Sync mode: compare versions, generate Replace if changed
/// - For Sync mode: generate Remove action for files no longer at source
///
/// # Arguments
/// * `discovered` - All discovered locations from the source
/// * `attached_files` - Map of source_location -> metadata for already-attached files
/// * `data_dir` - Data directory for computing relative paths
/// * `mode` - Sync mode (Add, Update, or Sync)
/// * `read_version` - Async function to read version from a URL
/// * `materialize` - Async function to materialize a discovered location (returns MaterializeResult with file and hash)
pub async fn process_sync_mode<M, MFut>(
    discovered: Vec<DiscoveredLocation>,
    attached_files: &HashMap<String, AttachedFileInfo>,
    data_dir: &dyn IOReadWriteDir,
    mode: SyncMode,
    read_version: impl Fn(&Url) -> BoxFuture<'_, Result<String, BundlebaseError>>,
    materialize: M,
) -> Result<Vec<FetchAction>, BundlebaseError>
where
    M: Fn(DiscoveredLocation) -> MFut,
    MFut: Future<Output = Result<MaterializeResult, BundlebaseError>>,
{
    // Build set of discovered source_locations for Remove detection
    let discovered_locations: HashSet<String> = discovered
        .iter()
        .map(|d| d.source_location.clone())
        .collect();

    let mut actions = Vec::new();

    for location in discovered {
        let source_location = location.source_location.clone();
        let source_url = location.url.to_string();

        if let Some(attached_info) = attached_files.get(&source_location) {
            // Already attached - check for changes in Update/Sync mode
            if mode == SyncMode::Update || mode == SyncMode::Sync {
                let current_version = read_version(&location.url).await?;
                if current_version != attached_info.version {
                    debug!(
                        "File {} changed: version {} -> {}",
                        source_location, attached_info.version, current_version
                    );
                    let result = materialize(location).await?;
                    // Use relative path if file is in data_dir, otherwise full URL
                    let attach_location = data_dir
                        .relative_path(result.file.as_ref())
                        .unwrap_or_else(|_| result.file.url().to_string());
                    actions.push(FetchAction::Replace {
                        old_source_location: source_location.clone(),
                        data: MaterializedData {
                            attach_location,
                            source_location,
                            source_url,
                            hash: result.hash,
                        },
                    });
                }
            }
            // For Add mode, skip files that are already attached
        } else {
            // New file - add it
            let result = materialize(location).await?;
            // Use relative path if file is in data_dir, otherwise full URL
            let attach_location = data_dir
                .relative_path(result.file.as_ref())
                .unwrap_or_else(|_| result.file.url().to_string());
            actions.push(FetchAction::Add(MaterializedData {
                attach_location,
                source_location,
                source_url,
                hash: result.hash,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_patterns_single() {
        let patterns = parse_patterns("*.parquet").unwrap();
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("file.parquet"));
        assert!(!patterns[0].matches("file.csv"));
    }

    #[test]
    fn test_parse_patterns_multiple() {
        let patterns = parse_patterns("*.parquet, *.csv").unwrap();
        assert_eq!(patterns.len(), 2);
        assert!(patterns[0].matches("file.parquet"));
        assert!(patterns[1].matches("file.csv"));
    }

    #[test]
    fn test_parse_patterns_invalid() {
        let result = parse_patterns("[invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_patterns_empty_parts() {
        let patterns = parse_patterns("*.parquet,,*.csv").unwrap();
        assert_eq!(patterns.len(), 2);
    }

    #[test]
    fn test_get_patterns_default() {
        let args = HashMap::new();
        let patterns = get_patterns(&args).unwrap();
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("anything"));
    }

    #[test]
    fn test_get_patterns_custom() {
        let mut args = HashMap::new();
        args.insert("patterns".to_string(), "*.csv".to_string());
        let patterns = get_patterns(&args).unwrap();
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("file.csv"));
        assert!(!patterns[0].matches("file.parquet"));
    }

    #[test]
    fn test_matches_patterns_filename() {
        let patterns = parse_patterns("*.parquet").unwrap();
        let url = Url::parse("https://example.com/data/file.parquet").unwrap();
        assert!(matches_patterns(&url, &patterns));
    }

    #[test]
    fn test_matches_patterns_path() {
        let patterns = parse_patterns("data/*.parquet").unwrap();
        let url = Url::parse("https://example.com/data/file.parquet").unwrap();
        assert!(matches_patterns(&url, &patterns));
    }

    #[test]
    fn test_matches_patterns_no_match() {
        let patterns = parse_patterns("*.csv").unwrap();
        let url = Url::parse("https://example.com/data/file.parquet").unwrap();
        assert!(!matches_patterns(&url, &patterns));
    }

    #[test]
    fn test_should_copy_default() {
        let args = HashMap::new();
        assert!(should_copy(&args));
    }

    #[test]
    fn test_should_copy_true() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "true".to_string());
        assert!(should_copy(&args));
    }

    #[test]
    fn test_should_copy_false() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "false".to_string());
        assert!(!should_copy(&args));
    }

    #[test]
    fn test_validate_copy_arg_valid() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "true".to_string());
        assert!(validate_copy_arg("test", &args).is_ok());

        args.insert("copy".to_string(), "false".to_string());
        assert!(validate_copy_arg("test", &args).is_ok());
    }

    #[test]
    fn test_validate_copy_arg_invalid() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "invalid".to_string());
        let result = validate_copy_arg("test", &args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("'copy' argument must be"));
    }

    #[test]
    fn test_require_arg_present() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com".to_string());
        let result = require_arg(&args, "url", "test");
        assert_eq!(result.unwrap(), "https://example.com");
    }

    #[test]
    fn test_require_arg_missing() {
        let args = HashMap::new();
        let result = require_arg(&args, "url", "test");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires a 'url' argument"));
    }

    #[test]
    fn test_require_url_valid() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com/data/".to_string());
        let result = require_url(&args, "test");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "https://example.com/data/");
    }

    #[test]
    fn test_require_url_invalid() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "not-a-url".to_string());
        let result = require_url(&args, "test");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid URL"));
    }

    #[test]
    fn test_filename_from_url() {
        let url = Url::parse("https://example.com/data/file.parquet").unwrap();
        assert_eq!(filename_from_url(&url), "file.parquet");
    }

    #[test]
    fn test_filename_from_url_no_filename() {
        let url = Url::parse("https://example.com/").unwrap();
        assert_eq!(filename_from_url(&url), "data");
    }

    #[test]
    fn test_filename_from_url_nested() {
        let url = Url::parse("s3://bucket/path/to/data.csv").unwrap();
        assert_eq!(filename_from_url(&url), "data.csv");
    }
}
