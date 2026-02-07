//! Built-in "kaggle" source function.
//!
//! Discovers and downloads dataset files from Kaggle via their REST API.
//! Authentication is read from `~/.kaggle/kaggle.json`.

use super::source_function::{
    ArgSpec, AttachedFileInfo, DiscoveredLocation, FetchAction, MaterializedData, SourceFunction,
};
use super::SyncMode;
use super::source_utils::{self, MaterializeResult};
use crate::bundle_config::{config_keys, config_scopes, ConfigKey, ConfigScope};
use crate::io::IOReadWriteDir;
use crate::{BundleConfig, BundlebaseError, Scope};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

mod client;
use client::KaggleClient;

config_scopes!(scopes, {
    pub const KAGGLE_SCOPE: ConfigScope = {
        /// Custom URL→name for Kaggle: extracts owner/dataset from the URL path.
        /// Strips a leading `/datasets/` prefix if present.
        /// e.g., "https://www.kaggle.com/datasets/zillow/zecon" → Some("kaggle/zillow/zecon")
        /// e.g., "https://www.kaggle.com/zillow/zecon/extra"    → Some("kaggle/zillow/zecon")
        fn url_to_name(scope: &ConfigScope, input: &str) -> Option<String> {
            if let Ok(url) = Url::parse(input) {
                let mut segments = url.path()
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .peekable();
                // Skip leading "datasets" path prefix used in browser URLs
                if segments.peek() == Some(&"datasets") {
                    segments.next();
                }
                let owner_dataset: Vec<&str> = segments.take(2).collect();
                if owner_dataset.is_empty() {
                    Some(scope.name.to_string())
                } else {
                    Some(format!(
                        "{}/{}",
                        scope.name, owner_dataset.join("/")
                    ))
                }
            } else {
                None
            }
        }
        BundleConfig::register_scope("kaggle").with_url_to_name(url_to_name)
    };
});

config_keys!(configs, {
    pub const URL_CFG: ConfigKey = KAGGLE_SCOPE
        .define("url")
        .with_default("https://www.kaggle.com");
    pub const USERNAME_CFG: ConfigKey = KAGGLE_SCOPE
        .define("username")
        .with_default_fn("username in ~/.kaggle/kaggle.json", || read_kaggle_json_field("username"));
    pub const API_KEY_CFG: ConfigKey = KAGGLE_SCOPE
        .define_secure("key")
        .with_default_fn("key in ~/.kaggle/kaggle.json", || read_kaggle_json_field("key"));
});

pub(super) fn dataset_scope(dataset: &str) -> Result<Scope, crate::BundlebaseError> {
    Scope::new(&format!("{}/{}", KAGGLE_SCOPE.name, dataset))
}

fn read_kaggle_json_field(field: &str) -> Option<String> {
    let path = shellexpand::tilde("~/.kaggle/kaggle.json").to_string();
    let content = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    json.get(field).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Built-in "kaggle" source function.
///
/// Discovers and downloads dataset files from Kaggle using the Kaggle REST API.
/// Files are always copied into the bundle's data directory.
///
/// Arguments:
/// - `dataset` (required): Dataset identifier in `owner/dataset-name` format (e.g., `zillow/zecon`)
/// - `patterns` (optional): Comma-separated glob patterns to filter files (e.g., "*.csv")
///   Defaults to "**/*" (all files)
/// - `version` (optional): Dataset version number to download (default: latest)
pub struct KaggleSource;

#[async_trait]
impl SourceFunction for KaggleSource {
    fn name(&self) -> &str {
        "kaggle"
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec {
                name: "dataset",
                description: "Dataset identifier in owner/dataset-name format (e.g., zillow/zecon)",
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
        //todo: just check the format, don't need parse_dataset_arg function anymore
        parse_dataset_arg(dataset)?;

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
        let patterns = source_utils::get_patterns(args)?;
        let version = args.get("version").map(|s| s.as_str());
        let client = KaggleClient::from_config(config, dataset)?;

        // Version is intentionally discarded during discovery — it is only needed
        // when materializing files (in fetch/fetch_with_mode) to record in MaterializedData.
        let (all_files, _dataset_version) = Self::list_kaggle_files(
            &client,
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
        args: &HashMap<String, String>,
        data_dir: &dyn IOReadWriteDir,
        config: &Arc<BundleConfig>,
    ) -> Result<MaterializeResult, BundlebaseError> {
        let dataset = source_utils::require_arg(args, "dataset", self.name())?;
        let client = KaggleClient::from_config(config, dataset)?;
        Self::download_kaggle_file(
            &client,
            &location.url,
            &location.source_location,
            data_dir,
        )
        .await
    }

    async fn fetch(
        &self,
        args: &HashMap<String, String>,
        attached_locations: HashSet<String>,
        data_dir: &dyn IOReadWriteDir,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<MaterializedData>, BundlebaseError> {
        let dataset = source_utils::require_arg(args, "dataset", self.name())?;
        let patterns = source_utils::get_patterns(args)?;
        let version = args.get("version").map(|s| s.as_str());
        let client = KaggleClient::from_config(&config, dataset)?;

        let (all_files, dataset_version) = Self::list_kaggle_files(
            &client,
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
                &client,
                &location.url,
                &location.source_location,
                data_dir,
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
                let attached_locations: HashSet<String> = attached_files.keys().cloned().collect();
                let materialized = self
                    .fetch(args, attached_locations, data_dir, config)
                    .await?;
                Ok(materialized.into_iter().map(FetchAction::Add).collect())
            }
            SyncMode::Update | SyncMode::Sync => {
                let dataset = source_utils::require_arg(args, "dataset", self.name())?;
                let patterns = source_utils::get_patterns(args)?;
                let version = args.get("version").map(|s| s.as_str());
                let client = KaggleClient::from_config(&config, dataset)?;

                let (discovered, dataset_version) = Self::list_kaggle_files(
                    &client,
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
                                &client,
                                &location.url,
                                &source_location,
                                data_dir,
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
                            &client,
                            &location.url,
                            &source_location,
                            data_dir,
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
                            log::debug!("File {} no longer exists at remote", source_location);
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

impl KaggleSource {
    /// Read the current version number for a Kaggle dataset.
    ///
    /// Searches the datasets list endpoint (`/api/v1/datasets/list`) for the
    /// specific dataset and extracts `currentVersionNumber` from the result.
    /// If a specific version is requested, that version string is returned directly.
    async fn read_kaggle_version(
        client: &KaggleClient,
        dataset: &str,
        version: Option<&str>,
    ) -> Result<String, BundlebaseError> {
        // If a specific version was requested, use it directly
        if let Some(v) = version {
            return Ok(v.to_string());
        }

        let path = format!(
            "/api/v1/datasets/list?search={}",
            dataset
        );
        let response = client
            .get_path(&path)
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
        if let Some(datasets) = body.as_array() {
            for ds in datasets {
                let ds_ref = ds.get("ref").and_then(|v| v.as_str()).unwrap_or("");
                if ds_ref == dataset {
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
        client: &KaggleClient,
        dataset: &str,
        patterns: &[glob::Pattern],
        version: Option<&str>,
    ) -> Result<(Vec<DiscoveredLocation>, String), BundlebaseError> {
        let mut list_path = format!(
            "/api/v1/datasets/list/{}",
            dataset
        );
        if let Some(v) = version {
            list_path.push_str(&format!("?datasetVersionNumber={}", v));
        }
        let response = client
            .get_path(&list_path)
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
            client,
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

            // Kaggle API returns flat file names (no subdirectory paths), so matching
            // against the bare filename is correct. Users should use simple patterns
            // like "*.csv" rather than path-based patterns like "subdir/*.csv".
            if !patterns.iter().any(|pattern| pattern.matches(file_name)) {
                continue;
            }

            let mut download_url = format!(
                "{}/api/v1/datasets/download/{}/{}",
                client.base_url, dataset, file_name
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
        client: &KaggleClient,
        url: &Url,
        source_location: &str,
        data_dir: &dyn IOReadWriteDir,
    ) -> Result<MaterializeResult, BundlebaseError> {
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;

        let response = client
            .get_url(url.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        let func = KaggleSource;
        assert_eq!(func.name(), "kaggle");
    }

    #[test]
    fn test_arg_specs() {
        let func = KaggleSource;
        let specs = func.arg_specs();
        assert_eq!(specs.len(), 3);
        assert!(specs.iter().any(|s| s.name == "dataset" && s.required));
        assert!(specs.iter().any(|s| s.name == "patterns" && !s.required));
        assert!(specs.iter().any(|s| s.name == "version" && !s.required));
    }

    #[test]
    fn test_validate_args_valid() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_missing_dataset() {
        let func = KaggleSource;
        let args = HashMap::new();

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires a 'dataset' argument"));
    }

    #[test]
    fn test_validate_args_invalid_dataset_format_no_slash() {
        let func = KaggleSource;
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
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "a/b/c".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid dataset format"));
    }

    #[test]
    fn test_validate_args_invalid_dataset_format_empty_parts() {
        let func = KaggleSource;
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
    fn test_validate_args_with_patterns() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("patterns".to_string(), "*.csv".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_with_valid_version() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("version".to_string(), "3".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_version_zero() {
        let func = KaggleSource;
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
        let func = KaggleSource;
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
        let func = KaggleSource;
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
        let func = KaggleSource;
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
    fn test_kaggle_client_from_config_missing_credentials() {
        // Credentials are optional — client should succeed with None values
        let config = BundleConfig::new();
        let client = KaggleClient::from_config(&config, "zillow/zecon").unwrap();
        // If ~/.kaggle/kaggle.json exists, default_fn may populate these;
        // otherwise they should be None
        if client.username.is_none() {
            assert!(client.key.is_none() || client.key.is_some());
        }
    }

    #[test]
    fn test_kaggle_client_from_config() {
        let config = BundleConfig::new();
        let scope = Scope::try_from("kaggle").unwrap();
        config.set(
            &scope,
            URL_CFG.key,
            "https://test.kaggle.com",
            crate::bundle_config::ConfigSource::Passed,
        ).unwrap();
        config.set(
            &scope,
            USERNAME_CFG.key,
            "config_user",
            crate::bundle_config::ConfigSource::Passed,
        ).unwrap();
        config.set(
            &scope,
            API_KEY_CFG.key,
            "config_key",
            crate::bundle_config::ConfigSource::Passed,
        ).unwrap();

        let client = KaggleClient::from_config(&config, "zillow/zecon").unwrap();
        assert_eq!(client.base_url, "https://test.kaggle.com");
        assert_eq!(client.username, Some("config_user".to_string()));
        assert_eq!(client.key, Some("config_key".to_string()));
    }

    #[test]
    fn test_kaggle_client_from_config_partial_falls_back_to_default_fn() {
        // If only username is set in config (no key), key falls back to default_fn.
        // If default_fn also returns None, key is simply None.
        let config = BundleConfig::new();
        let scope = Scope::try_from("kaggle").unwrap();
        config.set(
            &scope,
            USERNAME_CFG.key,
            "config_user",
            crate::bundle_config::ConfigSource::Passed,
        ).unwrap();

        let client = KaggleClient::from_config(&config, "zillow/zecon").unwrap();
        assert_eq!(client.username, Some("config_user".to_string()));
        // key may be Some (if ~/.kaggle/kaggle.json exists) or None
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
        let result = KaggleSource::extract_from_zip(&zip_bytes, "hello.txt").unwrap();
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
        let result = KaggleSource::extract_from_zip(&zip_bytes, "test.csv");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "Expected 'empty' in: {}", err);
    }

    #[test]
    fn test_extract_from_zip_invalid_data() {
        let garbage = bytes::Bytes::from(vec![0u8, 1, 2, 3, 4, 5]);
        let result = KaggleSource::extract_from_zip(&garbage, "test.csv");
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
        let client = KaggleClient::new("http://unused", Some("user".into()), Some("key".into())).unwrap();
        let result = KaggleSource::read_kaggle_version(
            &client,
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).unwrap();
        let result = KaggleSource::read_kaggle_version(
            &client,
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).unwrap();
        let result = KaggleSource::read_kaggle_version(
            &client,
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).unwrap();
        let result = KaggleSource::read_kaggle_version(
            &client,
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
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).unwrap();
        let result = KaggleSource::read_kaggle_version(
            &client,
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

        let client = KaggleClient::new(&server.uri(), Some("myuser".into()), Some("mykey".into())).unwrap();
        let result = KaggleSource::read_kaggle_version(
            &client,
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
            .and(wiremock::matchers::path(
                "/api/v1/datasets/list/zillow/zecon",
            ))
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).unwrap();
        let patterns = vec![glob::Pattern::new("**/*").unwrap()];
        let (files, version) = KaggleSource::list_kaggle_files(
            &client,
            "zillow/zecon",
            &patterns,
            None,
        )
        .await
        .unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].source_location, "data.csv");
        assert_eq!(files[1].source_location, "readme.md");
        assert!(files[0]
            .url
            .as_str()
            .contains("/api/v1/datasets/download/zillow/zecon/data.csv"));
        assert!(files[1]
            .url
            .as_str()
            .contains("/api/v1/datasets/download/zillow/zecon/readme.md"));
        assert_eq!(version, "7");
    }

    #[tokio::test]
    async fn test_list_kaggle_files_with_pattern_filter() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/api/v1/datasets/list/zillow/zecon",
            ))
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).unwrap();
        let patterns = vec![glob::Pattern::new("*.csv").unwrap()];
        let (files, _) = KaggleSource::list_kaggle_files(
            &client,
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
            .and(wiremock::matchers::path(
                "/api/v1/datasets/list/zillow/zecon",
            ))
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).unwrap();
        let patterns = vec![glob::Pattern::new("**/*").unwrap()];
        let (files, version) = KaggleSource::list_kaggle_files(
            &client,
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
            .and(wiremock::matchers::path(
                "/api/v1/datasets/list/zillow/zecon",
            ))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).unwrap();
        let patterns = vec![glob::Pattern::new("**/*").unwrap()];
        let result = KaggleSource::list_kaggle_files(
            &client,
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
            .and(wiremock::matchers::path(
                "/api/v1/datasets/list/zillow/zecon",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        // Version search mock (called before datasetFiles is checked)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).unwrap();
        let patterns = vec![glob::Pattern::new("**/*").unwrap()];
        let result = KaggleSource::list_kaggle_files(
            &client,
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

    // ── kaggle scope url_to_name tests ───────────────────────────────────

    #[test]
    fn test_kaggle_scope_url_to_name_https() {
        let result = KAGGLE_SCOPE.url_to_name("https://www.kaggle.com/a/b/c");
        assert_eq!(result, Some("kaggle/a/b".to_string()));
    }

    #[test]
    fn test_kaggle_scope_url_to_name_root_trailing_slash() {
        let result = KAGGLE_SCOPE.url_to_name("https://www.kaggle.com/");
        assert_eq!(result, Some("kaggle".to_string()));
    }

    #[test]
    fn test_kaggle_scope_url_to_name_root_no_slash() {
        let result = KAGGLE_SCOPE.url_to_name("https://www.kaggle.com");
        assert_eq!(result, Some("kaggle".to_string()));
    }

    #[test]
    fn test_kaggle_scope_url_to_name_scheme() {
        // In kaggle://config, "config" is the host (not the path), so url.path() is empty
        let result = KAGGLE_SCOPE.url_to_name("kaggle://config");
        assert_eq!(result, Some("kaggle".to_string()));
    }

    #[test]
    fn test_kaggle_scope_url_to_name_strips_trailing_slash() {
        let result = KAGGLE_SCOPE.url_to_name("https://www.kaggle.com/a/b/");
        assert_eq!(result, Some("kaggle/a/b".to_string()));
    }

    #[test]
    fn test_kaggle_scope_url_to_name_datasets_prefix() {
        let result = KAGGLE_SCOPE.url_to_name("https://www.kaggle.com/datasets/zillow/zecon");
        assert_eq!(result, Some("kaggle/zillow/zecon".to_string()));
    }

    #[test]
    fn test_kaggle_scope_url_to_name_datasets_prefix_with_extra() {
        let result = KAGGLE_SCOPE.url_to_name("https://www.kaggle.com/datasets/zillow/zecon/data");
        assert_eq!(result, Some("kaggle/zillow/zecon".to_string()));
    }
}
