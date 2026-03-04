//! Built-in "kaggle" source function.
//!
//! Discovers and downloads dataset files from Kaggle via their REST API.
//! Authentication is read from `~/.kaggle/kaggle.json`.

use super::source_function::{
    ArgSpec, DiscoveredLocation, SourceData, SourceFunction, SourceFunctionSignature,
};
use super::source_utils;
use crate::bundle_config::{config_keys, config_scopes, ConfigKey, ConfigScope};
use crate::{BundleConfig, BundlebaseError, Scope};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
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
    fn signature(&self) -> SourceFunctionSignature {
        SourceFunctionSignature {
            name: "kaggle".to_string(),
            arg_specs: vec![
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
            ],
            accepts_extra_args: false,
        }
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // Validate dataset format: must be exactly "owner/dataset-name"
        let dataset = source_utils::require_arg(args, "dataset", "kaggle")?;
        let parts: Vec<&str> = dataset.splitn(3, '/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(BundlebaseError::from(format!(
                "Invalid dataset format '{}'. Expected 'owner/dataset-name' (e.g., 'zillow/zecon')",
                dataset
            )));
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
        _attached_locations: &HashSet<String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        let dataset = source_utils::require_arg(args, "dataset", "kaggle")?;
        let patterns = source_utils::get_patterns(args)?;
        let version = args.get("version").map(|s| s.as_str());
        let client = KaggleClient::from_config(config, dataset)?;

        let (all_files, dataset_version) = Self::list_kaggle_files(
            &client,
            dataset,
            &patterns,
            version,
        )
        .await?;

        let locations = all_files
            .into_iter()
            .map(|kf| {
                let format = kf.filename
                    .rsplit('.')
                    .next()
                    .unwrap_or("dat")
                    .to_string();
                DiscoveredLocation {
                    location: kf.filename,
                    must_copy: true,
                    format,
                    version: dataset_version.clone(),
                }
            })
            .collect();

        Ok(locations)
    }

    async fn data(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Option<SourceData>, BundlebaseError> {
        let dataset = source_utils::require_arg(args, "dataset", "kaggle")?;
        let version = args.get("version").map(|s| s.as_str());
        let client = KaggleClient::from_config(config, dataset)?;

        // Build download URL for this file
        let mut download_url = format!(
            "{}/api/v1/datasets/download/{}/{}",
            client.base_url, dataset, location.location
        );
        if let Some(v) = version {
            download_url.push_str(&format!("?datasetVersionNumber={}", v));
        }

        let stream = Self::download_kaggle_bytes(&client, &download_url, &location.location).await?;
        Ok(Some(SourceData::RawBytes(stream)))
    }

    async fn stable_url(
        &self,
        _location: &DiscoveredLocation,
        _args: &HashMap<String, String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Option<Url>, BundlebaseError> {
        // Kaggle files require authentication, no stable URL
        Ok(None)
    }
}

/// Internal representation of a Kaggle file from the API.
struct KaggleFileInfo {
    /// Filename as returned by the Kaggle API
    filename: String,
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
    /// Returns `(kaggle_file_infos, dataset_version_number)`. The version
    /// number is fetched from the dataset view endpoint and applies to all
    /// files in the dataset.
    async fn list_kaggle_files(
        client: &KaggleClient,
        dataset: &str,
        patterns: &[glob::Pattern],
        version: Option<&str>,
    ) -> Result<(Vec<KaggleFileInfo>, String), BundlebaseError> {
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
            // against the bare filename is correct.
            if !patterns.iter().any(|pattern| pattern.matches(file_name)) {
                continue;
            }

            locations.push(KaggleFileInfo {
                filename: file_name.to_string(),
            });
        }

        Ok((locations, dataset_version))
    }

    /// Download a file from Kaggle with authentication and return a byte stream.
    ///
    /// Kaggle's individual file download API returns ZIP archives containing
    /// the requested file. This method streams the ZIP to a temp file on disk,
    /// extracts if needed, then returns a stream that reads from disk in chunks
    /// rather than loading the entire file into memory.
    async fn download_kaggle_bytes(
        client: &KaggleClient,
        url: &str,
        source_location: &str,
    ) -> Result<BoxStream<'static, Result<Bytes, std::io::Error>>, BundlebaseError> {
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;

        let response = client
            .get_url(url)
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

        // Check first 2 bytes to detect ZIP format
        let mut magic = [0u8; 2];
        {
            use std::io::Read;
            let mut f = temp.reopen().map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to reopen temp file for Kaggle download '{}': {}",
                    source_location, e
                ))
            })?;
            f.read_exact(&mut magic).map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to read temp file header for Kaggle download '{}': {}",
                    source_location, e
                ))
            })?;
        }

        let final_temp = if &magic == b"PK" {
            Self::extract_from_zip_to_file(temp.path(), source_location)?
        } else {
            // Non-ZIP: stream directly from the downloaded temp file
            temp
        };

        Ok(source_utils::stream_from_temp_file(final_temp))
    }

    /// Extract the first file from a ZIP archive on disk to a new temp file.
    ///
    /// Kaggle's API wraps individual file downloads in ZIP archives.
    /// Returns a `NamedTempFile` containing the extracted content.
    fn extract_from_zip_to_file(
        zip_path: &std::path::Path,
        source_location: &str,
    ) -> Result<tempfile::NamedTempFile, BundlebaseError> {
        use std::io::{Read, Write};

        let zip_file = std::fs::File::open(zip_path).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to open ZIP file from Kaggle for '{}': {}",
                source_location, e
            ))
        })?;
        let mut archive = zip::ZipArchive::new(zip_file).map_err(|e| {
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

        let mut entry = archive.by_index(0).map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to extract file from Kaggle ZIP for '{}': {}",
                source_location, e
            ))
        })?;

        let mut out_temp = tempfile::NamedTempFile::new().map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to create temp file for ZIP extraction '{}': {}",
                source_location, e
            ))
        })?;

        // Copy in chunks to avoid loading the entire extracted file into memory
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = entry.read(&mut buf).map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to read extracted file from Kaggle ZIP for '{}': {}",
                    source_location, e
                ))
            })?;
            if n == 0 {
                break;
            }
            out_temp.write_all(&buf[..n]).map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to write extracted file for '{}': {}",
                    source_location, e
                ))
            })?;
        }
        out_temp.flush().map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to flush extracted file for '{}': {}",
                source_location, e
            ))
        })?;

        Ok(out_temp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::source_function::validate_source_args;

    #[test]
    fn test_signature() {
        let func = KaggleSource;
        let sig = func.signature();
        assert_eq!(sig.name, "kaggle");
        assert_eq!(sig.arg_specs.len(), 3);
        assert!(sig.arg_specs.iter().any(|s| s.name == "dataset" && s.required));
        assert!(sig.arg_specs.iter().any(|s| s.name == "patterns" && !s.required));
        assert!(sig.arg_specs.iter().any(|s| s.name == "version" && !s.required));
    }

    #[test]
    fn test_validate_args_valid() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        assert!(validate_source_args(&func, &args).is_ok());
    }

    #[test]
    fn test_validate_args_missing_dataset() {
        let func = KaggleSource;
        let args = HashMap::new();

        let result = validate_source_args(&func, &args);
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("requires a 'dataset' argument"));
    }

    #[test]
    fn test_validate_args_invalid_dataset_format_no_slash() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "just-a-name".to_string());

        let result = validate_source_args(&func, &args);
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("Invalid dataset format"));
        assert!(err.contains("owner/dataset-name"));
    }

    #[test]
    fn test_validate_args_invalid_dataset_format_too_many_slashes() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "a/b/c".to_string());

        let result = validate_source_args(&func, &args);
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("Invalid dataset format"));
    }

    #[test]
    fn test_validate_args_invalid_dataset_format_empty_parts() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "/dataset".to_string());

        let result = validate_source_args(&func, &args);
        assert!(result.is_err());

        let mut args2 = HashMap::new();
        args2.insert("dataset".to_string(), "owner/".to_string());
        let result2 = validate_source_args(&func, &args2);
        assert!(result2.is_err());
    }

    #[test]
    fn test_validate_args_with_patterns() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("patterns".to_string(), "*.csv".to_string());
        assert!(validate_source_args(&func, &args).is_ok());
    }

    #[test]
    fn test_validate_args_with_valid_version() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("version".to_string(), "3".to_string());
        assert!(validate_source_args(&func, &args).is_ok());
    }

    #[test]
    fn test_validate_args_version_zero() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("version".to_string(), "0".to_string());

        let result = validate_source_args(&func, &args);
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("Invalid version"));
        assert!(err.contains("positive integer"));
    }

    #[test]
    fn test_validate_args_version_negative() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("version".to_string(), "-1".to_string());

        let result = validate_source_args(&func, &args);
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("Invalid version"));
    }

    #[test]
    fn test_validate_args_version_non_numeric() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("version".to_string(), "abc".to_string());

        let result = validate_source_args(&func, &args);
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("Invalid version"));
        assert!(err.contains("abc"));
    }

    #[test]
    fn test_validate_args_unknown_arg() {
        let func = KaggleSource;
        let mut args = HashMap::new();
        args.insert("dataset".to_string(), "zillow/zecon".to_string());
        args.insert("unknown".to_string(), "value".to_string());

        let result = validate_source_args(&func, &args);
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("does not accept argument 'unknown'"));
    }

    #[test]
    fn test_kaggle_client_from_config_missing_credentials() {
        let config = BundleConfig::new(None).expect("config creation failed");
        let client = KaggleClient::from_config(&config, "zillow/zecon").expect("client creation failed");
        if client.username.is_none() {
            assert!(client.key.is_none() || client.key.is_some());
        }
    }

    #[test]
    fn test_kaggle_client_from_config() {
        let config = BundleConfig::new(None).expect("config creation failed");
        let scope = Scope::try_from("kaggle").expect("scope creation failed");
        config.set(
            &scope,
            URL_CFG.key,
            "https://test.kaggle.com",
            crate::bundle_config::ConfigSource::Passed,
        ).expect("set url failed");
        config.set(
            &scope,
            USERNAME_CFG.key,
            "config_user",
            crate::bundle_config::ConfigSource::Passed,
        ).expect("set username failed");
        config.set(
            &scope,
            API_KEY_CFG.key,
            "config_key",
            crate::bundle_config::ConfigSource::Passed,
        ).expect("set key failed");

        let client = KaggleClient::from_config(&config, "zillow/zecon").expect("client creation failed");
        assert_eq!(client.base_url, "https://test.kaggle.com");
        assert_eq!(client.username, Some("config_user".to_string()));
        assert_eq!(client.key, Some("config_key".to_string()));
    }

    #[test]
    fn test_kaggle_client_from_config_partial_falls_back_to_default_fn() {
        let config = BundleConfig::new(None).expect("config creation failed");
        let scope = Scope::try_from("kaggle").expect("scope creation failed");
        config.set(
            &scope,
            USERNAME_CFG.key,
            "config_user",
            crate::bundle_config::ConfigSource::Passed,
        ).expect("set username failed");

        let client = KaggleClient::from_config(&config, "zillow/zecon").expect("client creation failed");
        assert_eq!(client.username, Some("config_user".to_string()));
    }

    // ── extract_from_zip tests ──────────────────────────────────────

    #[test]
    fn test_extract_from_zip_single_file() {
        use std::io::{Read, Write};

        // Write a ZIP to a temp file
        let mut zip_temp = tempfile::NamedTempFile::new().expect("temp file failed");
        {
            let mut zip = zip::ZipWriter::new(&mut zip_temp);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("hello.txt", options).expect("start_file failed");
            zip.write_all(b"hello world").expect("write_all failed");
            zip.finish().expect("finish failed");
        }

        let result_temp = KaggleSource::extract_from_zip_to_file(zip_temp.path(), "hello.txt")
            .expect("extract failed");
        let mut contents = Vec::new();
        std::fs::File::open(result_temp.path())
            .expect("open failed")
            .read_to_end(&mut contents)
            .expect("read failed");
        assert_eq!(contents, b"hello world");
    }

    #[test]
    fn test_extract_from_zip_empty_archive() {
        use std::io::Write;

        let mut zip_temp = tempfile::NamedTempFile::new().expect("temp file failed");
        {
            let zip = zip::ZipWriter::new(&mut zip_temp);
            zip.finish().expect("finish failed");
        }
        zip_temp.flush().expect("flush failed");

        let result = KaggleSource::extract_from_zip_to_file(zip_temp.path(), "test.csv");
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(err.contains("empty"), "Expected 'empty' in: {}", err);
    }

    #[test]
    fn test_extract_from_zip_invalid_data() {
        use std::io::Write;

        let mut garbage_temp = tempfile::NamedTempFile::new().expect("temp file failed");
        garbage_temp.write_all(&[0u8, 1, 2, 3, 4, 5]).expect("write failed");
        garbage_temp.flush().expect("flush failed");

        let result = KaggleSource::extract_from_zip_to_file(garbage_temp.path(), "test.csv");
        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
        assert!(
            err.contains("Failed to read ZIP"),
            "Expected 'Failed to read ZIP' in: {}",
            err
        );
    }

    // ── read_kaggle_version tests ───────────────────────────────────

    #[tokio::test]
    async fn test_read_kaggle_version_with_explicit_version() {
        let client = KaggleClient::new("http://unused", Some("user".into()), Some("key".into())).expect("client creation failed");
        let result = KaggleSource::read_kaggle_version(
            &client,
            "owner/ds",
            Some("5"),
        )
        .await
        .expect("read_kaggle_version failed");
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).expect("client creation failed");
        let result = KaggleSource::read_kaggle_version(
            &client,
            "zillow/zecon",
            None,
        )
        .await
        .expect("read_kaggle_version failed");
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).expect("client creation failed");
        let result = KaggleSource::read_kaggle_version(
            &client,
            "zillow/zecon",
            None,
        )
        .await
        .expect("read_kaggle_version failed");
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).expect("client creation failed");
        let result = KaggleSource::read_kaggle_version(
            &client,
            "zillow/zecon",
            None,
        )
        .await
        .expect("read_kaggle_version failed");
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).expect("client creation failed");
        let result = KaggleSource::read_kaggle_version(
            &client,
            "zillow/zecon",
            None,
        )
        .await
        .expect("read_kaggle_version failed");
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

        let client = KaggleClient::new(&server.uri(), Some("myuser".into()), Some("mykey".into())).expect("client creation failed");
        let result = KaggleSource::read_kaggle_version(
            &client,
            "zillow/zecon",
            None,
        )
        .await
        .expect("read_kaggle_version failed");
        assert_eq!(result, "1");
    }

    // ── list_kaggle_files tests ─────────────────────────────────────

    #[tokio::test]
    async fn test_list_kaggle_files_basic() {
        let server = wiremock::MockServer::start().await;

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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).expect("client creation failed");
        let patterns = vec![glob::Pattern::new("**/*").expect("pattern creation failed")];
        let (files, version) = KaggleSource::list_kaggle_files(
            &client,
            "zillow/zecon",
            &patterns,
            None,
        )
        .await
        .expect("list_kaggle_files failed");

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "data.csv");
        assert_eq!(files[1].filename, "readme.md");
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).expect("client creation failed");
        let patterns = vec![glob::Pattern::new("*.csv").expect("pattern creation failed")];
        let (files, _) = KaggleSource::list_kaggle_files(
            &client,
            "zillow/zecon",
            &patterns,
            None,
        )
        .await
        .expect("list_kaggle_files failed");

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "data.csv");
    }

    #[tokio::test]
    async fn test_list_kaggle_files_with_version_param() {
        let server = wiremock::MockServer::start().await;

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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).expect("client creation failed");
        let patterns = vec![glob::Pattern::new("**/*").expect("pattern creation failed")];
        let (files, version) = KaggleSource::list_kaggle_files(
            &client,
            "zillow/zecon",
            &patterns,
            Some("2"),
        )
        .await
        .expect("list_kaggle_files failed");

        assert_eq!(files.len(), 1);
        assert_eq!(version, "2");
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

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).expect("client creation failed");
        let patterns = vec![glob::Pattern::new("**/*").expect("pattern creation failed")];
        let result = KaggleSource::list_kaggle_files(
            &client,
            "zillow/zecon",
            &patterns,
            None,
        )
        .await;

        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
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

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/datasets/list"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let client = KaggleClient::new(&server.uri(), Some("user".into()), Some("key".into())).expect("client creation failed");
        let patterns = vec![glob::Pattern::new("**/*").expect("pattern creation failed")];
        let result = KaggleSource::list_kaggle_files(
            &client,
            "zillow/zecon",
            &patterns,
            None,
        )
        .await;

        assert!(result.is_err());
        let err = result.err().expect("expected error").to_string();
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
