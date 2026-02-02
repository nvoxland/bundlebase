//! Built-in "kaggle" source function.
//!
//! Discovers and downloads dataset files from Kaggle via their REST API.
//! Authentication is read from `~/.kaggle/kaggle.json`.

use super::source_function::{
    ArgSpec, AttachedFileInfo, DiscoveredLocation, FetchAction, MaterializedData, SourceFunction,
    SyncMode,
};
use super::source_utils::{self, MaterializeResult};
use crate::bundle_config::ConfigKey;
use crate::io::IOReadWriteDir;
use crate::{BundleConfig, BundlebaseError};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Configuration keys for the Kaggle service.
pub static KAGGLE_CONFIG_SPECS: &[ConfigKey] = &[
    ConfigKey { key: "base_url", secure: false },
    ConfigKey { key: "username", secure: false },
    ConfigKey { key: "key", secure: true },
];

const DEFAULT_KAGGLE_BASE_URL: &str = "https://www.kaggle.com";
const KAGGLE_CONFIG_URL: &str = "kaggle://config";

/// Built-in "kaggle" source function.
///
/// Discovers and downloads dataset files from Kaggle using the Kaggle REST API.
/// Files are always copied into the bundle's data directory.
///
/// Arguments:
/// - `dataset` (required): Dataset identifier in `owner/dataset-name` format (e.g., `zillow/zecon`)
/// - `patterns` (optional): Comma-separated glob patterns to filter files (e.g., "*.csv")
///   Defaults to "**/*" (all files)
/// - `mode` (optional): Sync mode for fetch:
///   - "add" (default): Only attach new files
///   - "update": Add new files and replace changed files
///   - "sync": Add new, replace changed, and remove files no longer at source
/// - `version` (optional): Dataset version number to download (default: latest)
pub struct KaggleFunction;

/// Resolve the Kaggle API base URL from config.
/// Falls back to https://www.kaggle.com.
fn kaggle_base_url(config: &BundleConfig) -> String {
    let scope = crate::bundle_config::Scope::from_url(KAGGLE_CONFIG_URL);
    config
        .get("base_url", &scope)
        .unwrap_or_else(|| DEFAULT_KAGGLE_BASE_URL.to_string())
}

/// Read Kaggle API credentials.
///
/// Resolution order:
/// 1. BundleConfig `kaggle://` prefix (`username` + `key`)
/// 2. ~/.kaggle/kaggle.json file
fn read_kaggle_credentials(config: &BundleConfig) -> Result<(String, String), BundlebaseError> {
    let scope = crate::bundle_config::Scope::from_url(KAGGLE_CONFIG_URL);
    if let (Some(u), Some(k)) = (
        config.get("username", &scope),
        config.get("key", &scope),
    ) {
        return Ok((u, k));
    }
    // Fall back to file-based credentials
    read_kaggle_credentials_from_file()
}

/// Read Kaggle API credentials from `~/.kaggle/kaggle.json`.
///
/// Returns `(username, key)` tuple.
fn read_kaggle_credentials_from_file() -> Result<(String, String), BundlebaseError> {
    let path = shellexpand::tilde("~/.kaggle/kaggle.json").to_string();
    let content = std::fs::read_to_string(&path).map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to read Kaggle credentials from '{}': {}. \
             Create this file with {{\"username\": \"YOUR_USERNAME\", \"key\": \"YOUR_API_KEY\"}} \
             or run 'kaggle' CLI setup.",
            path, e
        ))
    })?;

    let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to parse Kaggle credentials from '{}': {}",
            path, e
        ))
    })?;

    let username = json
        .get("username")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            BundlebaseError::from(format!(
                "Kaggle credentials file '{}' missing 'username' field",
                path
            ))
        })?
        .to_string();

    let key = json
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            BundlebaseError::from(format!(
                "Kaggle credentials file '{}' missing 'key' field",
                path
            ))
        })?
        .to_string();

    Ok((username, key))
}

/// Create an HTTP client for the Kaggle API with a bundlebase User-Agent.
///
/// Basic Auth is applied per-request via `.basic_auth()` rather than in default headers.
fn kaggle_client() -> Result<reqwest::Client, BundlebaseError> {
    use reqwest::header;

    let mut headers = header::HeaderMap::new();
    headers.insert(header::USER_AGENT, header::HeaderValue::from_static("bundlebase"));

    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|e| BundlebaseError::from(format!("Failed to create Kaggle HTTP client: {}", e)))
}

/// Parse the `dataset` argument into `(owner, dataset_name)`.
fn parse_dataset_arg(dataset: &str) -> Result<(&str, &str), BundlebaseError> {
    let parts: Vec<&str> = dataset.splitn(3, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(BundlebaseError::from(format!(
            "Invalid dataset format '{}'. Expected 'owner/dataset-name' (e.g., 'zillow/zecon')",
            dataset
        )));
    }
    Ok((parts[0], parts[1]))
}

#[async_trait]
impl SourceFunction for KaggleFunction {
    fn name(&self) -> &str {
        "kaggle"
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec {
                name: "dataset",
                description:
                    "Dataset identifier in owner/dataset-name format (e.g., zillow/zecon)",
                required: true,
                default: None,
            },
            ArgSpec {
                name: "patterns",
                description: "Comma-separated glob patterns to filter files",
                required: false,
                default: Some("**/*"),
            },
            ArgSpec {
                name: "mode",
                description: "Sync mode: 'add' (default), 'update', or 'sync'",
                required: false,
                default: Some("add"),
            },
            ArgSpec {
                name: "version",
                description: "Dataset version number to download (default: latest)",
                required: false,
                default: None,
            },
        ]
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        self.default_validate_args(args)?;

        // Validate dataset format
        let dataset = source_utils::require_arg(args, "dataset", self.name())?;
        parse_dataset_arg(dataset)?;

        // Validate mode if provided
        if let Some(mode) = args.get("mode") {
            SyncMode::from_arg(mode)?;
        }

        // Validate version if provided — must be a positive integer
        if let Some(version) = args.get("version") {
            match version.parse::<u64>() {
                Ok(0) => {
                    return Err(BundlebaseError::from(
                        "Invalid version '0'. Must be a positive integer (e.g., '1', '2', '3')"
                            .to_string(),
                    ));
                }
                Err(_) => {
                    return Err(BundlebaseError::from(format!(
                        "Invalid version '{}'. Must be a positive integer (e.g., '1', '2', '3')",
                        version
                    )));
                }
                Ok(_) => {} // valid
            }
        }

        Ok(())
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        let dataset = source_utils::require_arg(args, "dataset", self.name())?;
        let (owner, dataset_name) = parse_dataset_arg(dataset)?;
        let patterns = source_utils::get_patterns(args)?;
        let version = args.get("version").map(|s| s.as_str());
        let base_url = kaggle_base_url(config);
        let (username, key) = read_kaggle_credentials(config)?;

        let (all_files, _dataset_version) = Self::list_kaggle_files(
            &base_url,
            &username,
            &key,
            owner,
            dataset_name,
            dataset,
            &patterns,
            version,
        )
        .await?;

        // Filter out already attached files
        let locations = all_files
            .into_iter()
            .filter(|loc| !attached_locations.contains(&loc.source_location))
            .collect();

        Ok(locations)
    }

    async fn materialize(
        &self,
        location: &DiscoveredLocation,
        _args: &HashMap<String, String>,
        data_dir: &dyn IOReadWriteDir,
        config: &Arc<BundleConfig>,
    ) -> Result<MaterializeResult, BundlebaseError> {
        let (username, key) = read_kaggle_credentials(config)?;
        Self::download_kaggle_file(
            &location.url,
            &location.source_location,
            data_dir,
            &username,
            &key,
        )
        .await
    }

    /// Override fetch to use the Kaggle dataset version number instead of
    /// the local file's metadata for `source_info.version`.
    async fn fetch(
        &self,
        args: &HashMap<String, String>,
        attached_locations: HashSet<String>,
        data_dir: &dyn IOReadWriteDir,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<MaterializedData>, BundlebaseError> {
        let dataset = source_utils::require_arg(args, "dataset", self.name())?;
        let (owner, dataset_name) = parse_dataset_arg(dataset)?;
        let patterns = source_utils::get_patterns(args)?;
        let version = args.get("version").map(|s| s.as_str());
        let base_url = kaggle_base_url(&config);
        let (username, key) = read_kaggle_credentials(&config)?;

        let (all_files, dataset_version) = Self::list_kaggle_files(
            &base_url,
            &username,
            &key,
            owner,
            dataset_name,
            dataset,
            &patterns,
            version,
        )
        .await?;

        // Filter out already-attached locations
        let new_files: Vec<_> = all_files
            .into_iter()
            .filter(|loc| !attached_locations.contains(&loc.source_location))
            .collect();

        let mut results = Vec::with_capacity(new_files.len());
        for location in new_files {
            let source_url = location.url.to_string();
            let result = Self::download_kaggle_file(
                &location.url,
                &location.source_location,
                data_dir,
                &username,
                &key,
            )
            .await?;
            let attach_location = data_dir
                .relative_path(result.file.as_ref())
                .unwrap_or_else(|_| result.file.url().to_string());
            results.push(MaterializedData {
                attach_location,
                source_location: location.source_location,
                source_url,
                hash: result.hash,
                version: dataset_version.clone(),
            });
        }

        Ok(results)
    }

    /// Override fetch_with_mode to use the Kaggle dataset version number
    /// for change detection instead of per-file HTTP headers.
    async fn fetch_with_mode(
        &self,
        args: &HashMap<String, String>,
        attached_files: &HashMap<String, AttachedFileInfo>,
        data_dir: &dyn IOReadWriteDir,
        config: Arc<BundleConfig>,
        mode: SyncMode,
    ) -> Result<Vec<FetchAction>, BundlebaseError> {
        match mode {
            SyncMode::Add => {
                let attached_locations: HashSet<String> =
                    attached_files.keys().cloned().collect();
                let materialized =
                    self.fetch(args, attached_locations, data_dir, config).await?;
                Ok(materialized.into_iter().map(FetchAction::Add).collect())
            }
            SyncMode::Update | SyncMode::Sync => {
                let dataset = source_utils::require_arg(args, "dataset", self.name())?;
                let (owner, dataset_name) = parse_dataset_arg(dataset)?;
                let patterns = source_utils::get_patterns(args)?;
                let version = args.get("version").map(|s| s.as_str());
                let base_url = kaggle_base_url(&config);
                let (username, key) = read_kaggle_credentials(&config)?;

                let (discovered, dataset_version) = Self::list_kaggle_files(
                    &base_url,
                    &username,
                    &key,
                    owner,
                    dataset_name,
                    dataset,
                    &patterns,
                    version,
                )
                .await?;

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
                        // Already attached — compare dataset version
                        if dataset_version != attached_info.version {
                            log::debug!(
                                "File {} changed: version {} -> {}",
                                source_location,
                                attached_info.version,
                                dataset_version
                            );
                            let result = Self::download_kaggle_file(
                                &location.url,
                                &source_location,
                                data_dir,
                                &username,
                                &key,
                            )
                            .await?;
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
                                    version: dataset_version.clone(),
                                },
                            });
                        }
                    } else {
                        // New file — add it
                        let result = Self::download_kaggle_file(
                            &location.url,
                            &source_location,
                            data_dir,
                            &username,
                            &key,
                        )
                        .await?;
                        let attach_location = data_dir
                            .relative_path(result.file.as_ref())
                            .unwrap_or_else(|_| result.file.url().to_string());
                        actions.push(FetchAction::Add(MaterializedData {
                            attach_location,
                            source_location,
                            source_url,
                            hash: result.hash,
                            version: dataset_version.clone(),
                        }));
                    }
                }

                // For Sync mode: detect removed files
                if mode == SyncMode::Sync {
                    for source_location in attached_files.keys() {
                        if !discovered_locations.contains(source_location) {
                            log::debug!(
                                "File {} no longer exists at remote",
                                source_location
                            );
                            actions.push(FetchAction::Remove {
                                source_location: source_location.clone(),
                            });
                        }
                    }
                }

                Ok(actions)
            }
        }
    }
}

impl KaggleFunction {
    /// Read the current version number for a Kaggle dataset.
    ///
    /// Searches the datasets list endpoint (`/api/v1/datasets/list`) for the
    /// specific dataset and extracts `currentVersionNumber` from the result.
    /// If a specific version is requested, that version string is returned directly.
    async fn read_kaggle_version(
        base_url: &str,
        client: &reqwest::Client,
        username: &str,
        key: &str,
        owner: &str,
        dataset_name: &str,
        dataset: &str,
        version: Option<&str>,
    ) -> Result<String, BundlebaseError> {
        // If a specific version was requested, use it directly
        if let Some(v) = version {
            return Ok(v.to_string());
        }

        let search_url = format!(
            "{}/api/v1/datasets/list?search={}/{}",
            base_url, owner, dataset_name
        );
        let response = client
            .get(&search_url)
            .basic_auth(username, Some(key))
            .send()
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to read Kaggle dataset version for '{}': {}",
                    dataset, e
                ))
            })?;

        if !response.status().is_success() {
            log::warn!(
                "Failed to read Kaggle dataset version for '{}': HTTP {}. Using 'unknown'.",
                dataset,
                response.status()
            );
            return Ok("unknown".to_string());
        }

        let body_text = response.text().await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read Kaggle dataset version response for '{}': {}",
                dataset, e
            ))
        })?;
        let body: serde_json::Value = serde_json::from_str(&body_text).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to parse Kaggle dataset version response for '{}': {}",
                dataset, e
            ))
        })?;

        // Response is an array of dataset objects; find the matching one
        let dataset_ref = format!("{}/{}", owner, dataset_name);
        if let Some(datasets) = body.as_array() {
            for ds in datasets {
                let ds_ref = ds.get("ref").and_then(|v| v.as_str()).unwrap_or("");
                if ds_ref == dataset_ref {
                    if let Some(ver) = ds
                        .get("currentVersionNumber")
                        .and_then(|v| v.as_u64().map(|n| n.to_string()))
                    {
                        return Ok(ver);
                    }
                }
            }
        }

        log::warn!(
            "Could not find version number for dataset '{}' in search results. Using 'unknown'.",
            dataset
        );
        Ok("unknown".to_string())
    }

    /// List files in a Kaggle dataset, filtered by patterns.
    ///
    /// Returns `(discovered_locations, dataset_version_number)`. The version
    /// number is fetched from the dataset view endpoint and applies to all
    /// files in the dataset.
    async fn list_kaggle_files(
        base_url: &str,
        username: &str,
        key: &str,
        owner: &str,
        dataset_name: &str,
        dataset: &str,
        patterns: &[glob::Pattern],
        version: Option<&str>,
    ) -> Result<(Vec<DiscoveredLocation>, String), BundlebaseError> {
        let client = kaggle_client()?;

        let mut list_url = format!(
            "{}/api/v1/datasets/list/{}/{}",
            base_url, owner, dataset_name
        );
        if let Some(v) = version {
            list_url.push_str(&format!("?datasetVersionNumber={}", v));
        }
        let response = client
            .get(&list_url)
            .basic_auth(username, Some(key))
            .send()
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to list Kaggle dataset '{}': {}",
                    dataset, e
                ))
            })?;

        if !response.status().is_success() {
            return Err(BundlebaseError::from(format!(
                "Failed to list Kaggle dataset '{}': HTTP {}",
                dataset,
                response.status()
            )));
        }

        let body_text = response.text().await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read Kaggle API response for '{}': {}",
                dataset, e
            ))
        })?;
        let body: serde_json::Value = serde_json::from_str(&body_text).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to parse Kaggle API response for '{}': {}",
                dataset, e
            ))
        })?;

        // Fetch dataset version from the dataset view endpoint
        let dataset_version = Self::read_kaggle_version(
            base_url,
            &client,
            username,
            key,
            owner,
            dataset_name,
            dataset,
            version,
        )
        .await?;

        let files = body
            .get("datasetFiles")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                BundlebaseError::from(format!(
                    "Unexpected Kaggle API response for '{}': missing 'datasetFiles' array",
                    dataset
                ))
            })?;

        let mut locations = Vec::new();
        for file_entry in files {
            let file_name = match file_entry.get("name").and_then(serde_json::Value::as_str) {
                Some(name) => name,
                None => continue,
            };

            if !patterns.iter().any(|pattern| pattern.matches(file_name)) {
                continue;
            }

            let mut download_url = format!(
                "{}/api/v1/datasets/download/{}/{}/{}",
                base_url, owner, dataset_name, file_name
            );
            if let Some(v) = version {
                download_url.push_str(&format!("?datasetVersionNumber={}", v));
            }
            if let Ok(url) = Url::parse(&download_url) {
                locations.push(DiscoveredLocation {
                    url,
                    source_location: file_name.to_string(),
                });
            }
        }

        Ok((locations, dataset_version))
    }

    /// Download a file from Kaggle with authentication.
    ///
    /// Kaggle's individual file download API returns ZIP archives containing
    /// the requested file. This method streams the ZIP to a temp file on disk,
    /// then reads it back for extraction. This avoids holding both the ZIP and
    /// extracted content in memory simultaneously.
    async fn download_kaggle_file(
        url: &Url,
        source_location: &str,
        data_dir: &dyn IOReadWriteDir,
        username: &str,
        key: &str,
    ) -> Result<MaterializeResult, BundlebaseError> {
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;

        let client = kaggle_client()?;

        let response = client
            .get(url.as_str())
            .basic_auth(username, Some(key))
            .send()
            .await
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to download Kaggle file '{}': {}",
                    source_location, e
                ))
            })?;

        if !response.status().is_success() {
            return Err(BundlebaseError::from(format!(
                "Failed to download Kaggle file '{}': HTTP {}",
                source_location,
                response.status()
            )));
        }

        // Stream response to a temp file to avoid holding the full ZIP in memory
        let temp = tempfile::NamedTempFile::new().map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to create temp file for Kaggle download '{}': {}",
                source_location, e
            ))
        })?;
        let temp_path = temp.path().to_path_buf();
        {
            let std_file = temp.reopen().map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to reopen temp file for Kaggle download '{}': {}",
                    source_location, e
                ))
            })?;
            let mut file = tokio::fs::File::from_std(std_file);
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| {
                    BundlebaseError::from(format!(
                        "Failed to stream Kaggle file '{}': {}",
                        source_location, e
                    ))
                })?;
                file.write_all(&chunk).await.map_err(|e| {
                    BundlebaseError::from(format!(
                        "Failed to write temp file for Kaggle download '{}': {}",
                        source_location, e
                    ))
                })?;
            }
            file.flush().await.map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to flush temp file for Kaggle download '{}': {}",
                    source_location, e
                ))
            })?;
        }

        // Read temp file and extract from ZIP
        let zip_data = bytes::Bytes::from(std::fs::read(&temp_path).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read temp file for Kaggle download '{}': {}",
                source_location, e
            ))
        })?);
        let data = Self::extract_from_zip(&zip_data, source_location)?;
        drop(temp); // Clean up temp file

        let filename = source_location
            .rsplit('/')
            .next()
            .unwrap_or(source_location);
        let result = source_utils::download_to_data_dir(data, filename, data_dir).await?;
        Ok(MaterializeResult {
            file: result.file,
            hash: result.hash,
        })
    }

    /// Extract the first file from a ZIP archive.
    ///
    /// Kaggle's API wraps individual file downloads in ZIP archives.
    fn extract_from_zip(
        zip_data: &bytes::Bytes,
        source_location: &str,
    ) -> Result<bytes::Bytes, BundlebaseError> {
        use std::io::Read;

        let cursor = std::io::Cursor::new(zip_data.as_ref());
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read ZIP from Kaggle for '{}': {}",
                source_location, e
            ))
        })?;

        if archive.is_empty() {
            return Err(BundlebaseError::from(format!(
                "Kaggle ZIP for '{}' is empty",
                source_location
            )));
        }

        let mut file = archive.by_index(0).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to extract file from Kaggle ZIP for '{}': {}",
                source_location, e
            ))
        })?;

        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read extracted file from Kaggle ZIP for '{}': {}",
                source_location, e
            ))
        })?;

        Ok(bytes::Bytes::from(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        let func = KaggleFunction;
        assert_eq!(func.name(), "kaggle");
    }

    #[test]
    fn test_arg_specs() {
        let func = KaggleFunction;
        let specs = func.arg_specs();
        assert_eq!(specs.len(), 4);
        assert!(specs.iter().any(|s| s.name == "dataset" && s.required));
        assert!(specs.iter().any(|s| s.name == "patterns" && !s.required));
        assert!(specs.iter().any(|s| s.name == "mode" && !s.required));
        assert!(specs.iter().any(|s| s.name == "version" && !s.required));
    }

    #[test]
    fn test_validate_args_valid() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_missing_dataset() {
        let func = KaggleFunction;
        let args = HashMap::new();

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires a 'dataset' argument"));
    }

    #[test]
    fn test_validate_args_invalid_dataset_format_no_slash() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "just-a-name".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid dataset format"));
        assert!(err.contains("owner/dataset-name"));
    }

    #[test]
    fn test_validate_args_invalid_dataset_format_too_many_slashes() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "a/b/c".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid dataset format"));
    }

    #[test]
    fn test_validate_args_invalid_dataset_format_empty_parts() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "/dataset".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());

        let mut args2 = HashMap::new();
        args2.insert("dataset".to_string(), "owner/".to_string());
        let result2 = func.validate_args(&args2);
        assert!(result2.is_err());
    }

    #[test]
    fn test_validate_args_invalid_mode() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("mode".to_string(), "invalid".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid mode"));
    }

    #[test]
    fn test_validate_args_with_patterns() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("patterns".to_string(), "*.csv".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_with_mode() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("mode".to_string(), "sync".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_with_valid_version() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("version".to_string(), "3".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_version_zero() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("version".to_string(), "0".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid version"));
        assert!(err.contains("positive integer"));
    }

    #[test]
    fn test_validate_args_version_negative() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("version".to_string(), "-1".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid version"));
    }

    #[test]
    fn test_validate_args_version_non_numeric() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("version".to_string(), "abc".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid version"));
        assert!(err.contains("abc"));
    }

    #[test]
    fn test_validate_args_unknown_arg() {
        let func = KaggleFunction;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("unknown".to_string(), "value".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not accept argument 'unknown'"));
    }

    #[test]
    fn test_parse_dataset_arg_valid() {
        let (owner, name) = parse_dataset_arg("zillow/zecon").unwrap();
        assert_eq!(owner, "zillow");
        assert_eq!(name, "zecon");
    }

    #[test]
    fn test_parse_dataset_arg_no_slash() {
        let result = parse_dataset_arg("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_dataset_arg_too_many_slashes() {
        let result = parse_dataset_arg("a/b/c");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_dataset_arg_empty_owner() {
        let result = parse_dataset_arg("/dataset");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_dataset_arg_empty_name() {
        let result = parse_dataset_arg("owner/");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_credentials_missing_file() {
        // This test verifies that a clear error message is returned
        // when credentials file doesn't exist (which is likely in CI)
        let config = BundleConfig::new();
        let result = read_kaggle_credentials(&config);
        // In CI or when credentials aren't configured, this should fail gracefully
        if result.is_err() {
            let err = result.unwrap_err().to_string();
            assert!(err.contains("kaggle.json"));
            assert!(err.contains("username"));
        }
        // If credentials exist, that's fine too - the function works
    }

    #[test]
    fn test_read_credentials_from_config() {
        let config = BundleConfig::new();
        let scope = crate::bundle_config::Scope::from_url("kaggle://");
        config.set("username", "config_user", &scope, crate::bundle_config::ConfigSource::Passed);
        config.set("key", "config_key", &scope, crate::bundle_config::ConfigSource::Passed);

        let (username, key) = read_kaggle_credentials(&config).unwrap();
        assert_eq!(username, "config_user");
        assert_eq!(key, "config_key");
    }

    #[test]
    fn test_read_credentials_partial_config_falls_back_to_file() {
        // If only username is set in config (no key), should fall back to file
        let config = BundleConfig::new();
        let scope = crate::bundle_config::Scope::from_url("kaggle://");
        config.set("username", "config_user", &scope, crate::bundle_config::ConfigSource::Passed);

        let result = read_kaggle_credentials(&config);
        // Will either succeed (if ~/.kaggle/kaggle.json exists) or fail with file error
        if result.is_err() {
            let err = result.unwrap_err().to_string();
            assert!(err.contains("kaggle.json"));
        }
    }

    #[test]
    fn test_kaggle_base_url_default() {
        let config = BundleConfig::new();
        assert_eq!(kaggle_base_url(&config), "https://www.kaggle.com");
    }

    #[test]
    fn test_kaggle_base_url_from_config() {
        let config = BundleConfig::new();
        let scope = crate::bundle_config::Scope::from_url("kaggle://");
        config.set("base_url", "https://custom.kaggle.com", &scope, crate::bundle_config::ConfigSource::Passed);
        assert_eq!(kaggle_base_url(&config), "https://custom.kaggle.com");
    }

    // ── extract_from_zip tests ──────────────────────────────────────

    #[test]
    fn test_extract_from_zip_single_file() {
        use std::io::Write;

        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("hello.txt", options).unwrap();
            zip.write_all(b"hello world").unwrap();
            zip.finish().unwrap();
        }

        let zip_bytes = bytes::Bytes::from(buf);
        let result = KaggleFunction::extract_from_zip(&zip_bytes, "hello.txt").unwrap();
        assert_eq!(result.as_ref(), b"hello world");
    }

    #[test]
    fn test_extract_from_zip_empty_archive() {
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let zip = zip::ZipWriter::new(cursor);
            zip.finish().unwrap();
        }

        let zip_bytes = bytes::Bytes::from(buf);
        let result = KaggleFunction::extract_from_zip(&zip_bytes, "test.csv");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "Expected 'empty' in: {}", err);
    }

    #[test]
    fn test_extract_from_zip_invalid_data() {
        let garbage = bytes::Bytes::from(vec![0u8, 1, 2, 3, 4, 5]);
        let result = KaggleFunction::extract_from_zip(&garbage, "test.csv");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Failed to read ZIP"),
            "Expected 'Failed to read ZIP' in: {}",
            err
        );
    }

    // ── read_kaggle_version tests ───────────────────────────────────

    #[tokio::test]
    async fn test_read_kaggle_version_with_explicit_version() {
        let client = reqwest::Client::new();
        let result = KaggleFunction::read_kaggle_version(
            "http://unused",
            &client,
            "user",
            "key",
            "owner",
            "ds",
            "owner/ds",
            Some("5"),
        )
        .await
        .unwrap();
        assert_eq!(result, "5");
    }

    #[tokio::test]
    async fn test_read_kaggle_version_from_api() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .and(wiremock::matchers::query_param("search", "zillow/zecon"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    {"ref": "zillow/zecon", "currentVersionNumber": 42}
                ])),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = KaggleFunction::read_kaggle_version(
            &server.uri(),
            &client,
            "user",
            "key",
            "zillow",
            "zecon",
            "zillow/zecon",
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, "42");
    }

    #[tokio::test]
    async fn test_read_kaggle_version_dataset_not_in_results() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    {"ref": "other/dataset", "currentVersionNumber": 10}
                ])),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = KaggleFunction::read_kaggle_version(
            &server.uri(),
            &client,
            "user",
            "key",
            "zillow",
            "zecon",
            "zillow/zecon",
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, "unknown");
    }

    #[tokio::test]
    async fn test_read_kaggle_version_api_error() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = KaggleFunction::read_kaggle_version(
            &server.uri(),
            &client,
            "user",
            "key",
            "zillow",
            "zecon",
            "zillow/zecon",
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, "unknown");
    }

    #[tokio::test]
    async fn test_read_kaggle_version_empty_array() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([])),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = KaggleFunction::read_kaggle_version(
            &server.uri(),
            &client,
            "user",
            "key",
            "zillow",
            "zecon",
            "zillow/zecon",
            None,
        )
        .await
        .unwrap();
        assert_eq!(result, "unknown");
    }

    #[tokio::test]
    async fn test_read_kaggle_version_sends_basic_auth() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .and(wiremock::matchers::header_exists("Authorization"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    {"ref": "zillow/zecon", "currentVersionNumber": 1}
                ])),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let result = KaggleFunction::read_kaggle_version(
            &server.uri(),
            &client,
            "myuser",
            "mykey",
            "zillow",
            "zecon",
            "zillow/zecon",
            None,
        )
        .await
        .unwrap();
        // If the Authorization header wasn't sent, wiremock would not match and
        // reqwest would get a 404, causing the function to return "unknown".
        assert_eq!(result, "1");
    }

    // ── list_kaggle_files tests ─────────────────────────────────────

    #[tokio::test]
    async fn test_list_kaggle_files_basic() {
        let server = wiremock::MockServer::start().await;

        // Mock the file list endpoint
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list/zillow/zecon"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "datasetFiles": [
                        {"name": "data.csv"},
                        {"name": "readme.md"}
                    ]
                })),
            )
            .mount(&server)
            .await;

        // Mock the version search endpoint
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .and(wiremock::matchers::query_param("search", "zillow/zecon"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    {"ref": "zillow/zecon", "currentVersionNumber": 7}
                ])),
            )
            .mount(&server)
            .await;

        let patterns = vec![glob::Pattern::new("**/*").unwrap()];
        let (files, version) = KaggleFunction::list_kaggle_files(
            &server.uri(),
            "user",
            "key",
            "zillow",
            "zecon",
            "zillow/zecon",
            &patterns,
            None,
        )
        .await
        .unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].source_location, "data.csv");
        assert_eq!(files[1].source_location, "readme.md");
        assert!(files[0].url.as_str().contains("/api/v1/datasets/download/zillow/zecon/data.csv"));
        assert!(files[1].url.as_str().contains("/api/v1/datasets/download/zillow/zecon/readme.md"));
        assert_eq!(version, "7");
    }

    #[tokio::test]
    async fn test_list_kaggle_files_with_pattern_filter() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list/zillow/zecon"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "datasetFiles": [
                        {"name": "data.csv"},
                        {"name": "readme.md"},
                        {"name": "extra.json"}
                    ]
                })),
            )
            .mount(&server)
            .await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    {"ref": "zillow/zecon", "currentVersionNumber": 1}
                ])),
            )
            .mount(&server)
            .await;

        let patterns = vec![glob::Pattern::new("*.csv").unwrap()];
        let (files, _) = KaggleFunction::list_kaggle_files(
            &server.uri(),
            "user",
            "key",
            "zillow",
            "zecon",
            "zillow/zecon",
            &patterns,
            None,
        )
        .await
        .unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].source_location, "data.csv");
    }

    #[tokio::test]
    async fn test_list_kaggle_files_with_version_param() {
        let server = wiremock::MockServer::start().await;

        // File list endpoint should receive the version query param
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list/zillow/zecon"))
            .and(wiremock::matchers::query_param("datasetVersionNumber", "2"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "datasetFiles": [
                        {"name": "data.csv"}
                    ]
                })),
            )
            .mount(&server)
            .await;

        // No version search mock needed — explicit version skips the API call

        let patterns = vec![glob::Pattern::new("**/*").unwrap()];
        let (files, version) = KaggleFunction::list_kaggle_files(
            &server.uri(),
            "user",
            "key",
            "zillow",
            "zecon",
            "zillow/zecon",
            &patterns,
            Some("2"),
        )
        .await
        .unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(version, "2");
        // Download URL should also include version
        assert!(
            files[0].url.as_str().contains("datasetVersionNumber=2"),
            "Expected version in download URL: {}",
            files[0].url
        );
    }

    #[tokio::test]
    async fn test_list_kaggle_files_api_error() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list/zillow/zecon"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let patterns = vec![glob::Pattern::new("**/*").unwrap()];
        let result = KaggleFunction::list_kaggle_files(
            &server.uri(),
            "user",
            "key",
            "zillow",
            "zecon",
            "zillow/zecon",
            &patterns,
            None,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Failed to list"),
            "Expected 'Failed to list' in: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_list_kaggle_files_missing_dataset_files() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list/zillow/zecon"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({})),
            )
            .mount(&server)
            .await;

        // Version search mock (called before datasetFiles is checked)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([])),
            )
            .mount(&server)
            .await;

        let patterns = vec![glob::Pattern::new("**/*").unwrap()];
        let result = KaggleFunction::list_kaggle_files(
            &server.uri(),
            "user",
            "key",
            "zillow",
            "zecon",
            "zillow/zecon",
            &patterns,
            None,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing 'datasetFiles'"),
            "Expected \"missing 'datasetFiles'\" in: {}",
            err
        );
    }
}
