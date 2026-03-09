//! Built-in "remote_dir" connector.
//!
//! Lists files from a directory URL using the IO registry to support
//! any URL scheme (file, s3, gs, azure, ftp, sftp, tar, etc.).

use crate::source::connector::{
    ArgSpec, DiscoveredLocation, SourceData, Connector, ConnectorSignature,
};
use crate::source::connector_utils;
use crate::io::file::IOReadFile;
use crate::io::plugin::ftp::FtpFile;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::plugin::sftp::{parse_sftp_url, SftpClient};
use crate::io::io_registry;
use crate::{BundleConfig, BundlebaseError};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Built-in "remote_dir" connector.
///
/// Lists files from a directory URL using standard object store listing.
/// Supports glob patterns for filtering files.
///
/// Arguments:
/// - `url` (required): The directory URL to list (e.g., "s3://bucket/data/")
/// - `patterns` (optional): Comma-separated glob patterns (e.g., "**/*.parquet,**/*.csv")
///   Defaults to "**/*" (all files)
/// - `copy` (optional): "true" to copy files into bundle's data_dir (default),
///   "false" to reference files at their original URL
/// - `key_path` (optional): SSH key path for SFTP sources
pub struct RemoteDirConnector;

#[async_trait]
impl Connector for RemoteDirConnector {
    fn signature(&self) -> ConnectorSignature {
        ConnectorSignature {
            name: "remote_dir".to_string(),
            arg_specs: vec![
                ArgSpec {
                    name: "url",
                    description: "The directory URL to list (e.g., s3://bucket/data/)",
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
                    name: "copy",
                    description: "Whether to copy files into bundle's data directory",
                    required: false,
                    default: Some("true"),
                },
                ArgSpec {
                    name: "key_path",
                    description: "SSH key path for SFTP sources",
                    required: false,
                    default: None,
                },
            ],
            accepts_extra_args: false,
        }
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // Validate URL is parseable
        connector_utils::require_url(args, "remote_dir")?;
        Ok(())
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        _attached_locations: &HashSet<String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        let base_url = connector_utils::require_url(args, "remote_dir")?;
        let patterns = connector_utils::get_patterns(args)?;
        let must_copy = Self::must_copy(&base_url);

        // Use IORegistry to create lister for any URL scheme
        let lister = io_registry()
            .create_lister(&base_url, config.clone())
            .await?;
        let all_files = lister.list_files().await?;

        let mut locations = Vec::new();
        for file in all_files {
            let relative_path = Self::relative_path(&base_url, &file.url);
            // Check pattern match
            if !patterns
                .iter()
                .any(|pattern| pattern.matches(&relative_path))
            {
                continue;
            }

            // Get format from file extension
            let format = relative_path
                .rsplit('.')
                .next()
                .unwrap_or("dat")
                .to_string();

            // Read version from remote file
            let version = Self::read_remote_version(&file.url, config).await
                .unwrap_or_else(|_| "unknown".to_string());

            locations.push(DiscoveredLocation {
                location: relative_path,
                must_copy,
                format,
                version,
            });
        }

        Ok(locations)
    }

    async fn data(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Option<SourceData>, BundlebaseError> {
        let base_url = connector_utils::require_url(args, "remote_dir")?;
        let scheme = base_url.scheme();

        // Only provide data directly for special protocols (SFTP, FTP)
        match scheme {
            "sftp" => {
                let file_url = Self::full_url(&base_url, &location.location)?;
                let key_path = args.get("key_path").map(|s| s.as_str());
                let stream = Self::download_sftp_stream(&file_url, key_path).await?;
                Ok(Some(SourceData::RawBytes(stream)))
            }
            "ftp" => {
                let file_url = Self::full_url(&base_url, &location.location)?;
                let stream = Self::download_ftp_stream(&file_url, config).await?;
                Ok(Some(SourceData::RawBytes(stream)))
            }
            _ => Ok(None),
        }
    }

    async fn stable_url(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Option<Url>, BundlebaseError> {
        let base_url = connector_utils::require_url(args, "remote_dir")?;

        // No stable URL for special protocols (handled by data())
        if Self::must_copy(&base_url) {
            return Ok(None);
        }

        let file_url = Self::full_url(&base_url, &location.location)?;

        Ok(Some(file_url))
    }
}

impl RemoteDirConnector {
    /// Read the version string from a remote URL.
    ///
    /// Uses IOFile to get version (ETag/S3 version/mtime hash) from the remote file.
    async fn read_remote_version(
        url: &Url,
        config: &Arc<BundleConfig>,
    ) -> Result<String, BundlebaseError> {
        let io_file = ObjectStoreFile::from_url(url, config.clone())?;
        io_file.version().await
    }

    /// Get the relative path of a file URL compared to the source URL.
    fn relative_path(source_url: &Url, file_url: &Url) -> String {
        let source_path = source_url.path();
        let file_path = file_url.path();

        if let Some(stripped) = file_path.strip_prefix(source_path) {
            stripped.trim_start_matches('/').to_string()
        } else {
            file_path.to_string()
        }
    }

    /// Reconstruct a full URL from a base URL and a relative location path.
    ///
    /// Unlike `Url::join()`, this always appends the relative path to the base path
    /// regardless of whether the base URL ends with a slash.
    fn full_url(base_url: &Url, relative_path: &str) -> Result<Url, BundlebaseError> {
        let mut url = base_url.clone();
        let base_path = url.path().to_string();
        let separator = if base_path.ends_with('/') { "" } else { "/" };
        url.set_path(&format!("{}{}{}", base_path, separator, relative_path));
        Ok(url)
    }

    /// Download a file via SFTP to a temp file, returning a byte stream.
    async fn download_sftp_stream(
        url: &Url,
        key_path: Option<&str>,
    ) -> Result<BoxStream<'static, Result<Bytes, std::io::Error>>, BundlebaseError> {
        let (user, host, port, remote_path) = parse_sftp_url(url)?;
        let key_path_str = key_path.ok_or_else(|| {
            BundlebaseError::from(
                "SFTP source requires 'key_path' argument for downloading files",
            )
        })?;
        let key_path_expanded = shellexpand::tilde(key_path_str).to_string();

        let sftp =
            SftpClient::connect(&host, port, &user, std::path::Path::new(&key_path_expanded))
                .await?;

        // Stream from SFTP into a temp file
        let mut remote_file = sftp.open_file(&remote_path).await?;
        let temp = tempfile::NamedTempFile::new().map_err(|e| {
            BundlebaseError::from(format!("Failed to create temp file for SFTP download: {}", e))
        })?;
        let mut async_temp = tokio::fs::File::from_std(temp.reopen().map_err(|e| {
            BundlebaseError::from(format!("Failed to reopen temp file for SFTP download: {}", e))
        })?);
        tokio::io::copy(&mut remote_file, &mut async_temp).await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to download SFTP file '{}': {}",
                remote_path, e
            ))
        })?;

        sftp.close().await?;

        Ok(connector_utils::stream_from_temp_file(temp))
    }

    /// Download a file via FTP to a temp file, returning a byte stream.
    async fn download_ftp_stream(
        url: &Url,
        config: &Arc<BundleConfig>,
    ) -> Result<BoxStream<'static, Result<Bytes, std::io::Error>>, BundlebaseError> {
        let ftp_file = FtpFile::from_url(url, config.clone())?;
        let temp = ftp_file.download_to_temp_file().await?.ok_or_else(|| {
            BundlebaseError::from(format!("FTP file not found: {}", url))
        })?;
        Ok(connector_utils::stream_from_temp_file(temp))
    }

    fn must_copy(base_url: &Url) -> bool {
        base_url.scheme() == "sftp" || base_url.scheme() == "ftp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature() {
        let func = RemoteDirConnector;
        let sig = func.signature();
        assert_eq!(sig.name, "remote_dir");
        assert_eq!(sig.arg_specs.len(), 4);
        assert!(sig.arg_specs.iter().any(|s| s.name == "url" && s.required));
        assert!(sig
            .arg_specs
            .iter()
            .any(|s| s.name == "patterns" && !s.required));
        assert!(sig
            .arg_specs
            .iter()
            .any(|s| s.name == "copy" && !s.required));
        assert!(sig
            .arg_specs
            .iter()
            .any(|s| s.name == "key_path" && !s.required));
    }

    #[test]
    fn test_validate_args_with_url() {
        let func = RemoteDirConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_invalid_url() {
        let func = RemoteDirConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "not-a-valid-url".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("Invalid URL"));
    }

    #[test]
    fn test_relative_path() {
        let source_url = Url::parse("s3://bucket/data/").expect("valid url");
        let file_url = Url::parse("s3://bucket/data/subdir/file.parquet").expect("valid url");

        let relative = RemoteDirConnector::relative_path(&source_url, &file_url);
        assert_eq!(relative, "subdir/file.parquet");
    }

    #[test]
    fn test_relative_path_root() {
        let source_url = Url::parse("s3://bucket/data/").expect("valid url");
        let file_url = Url::parse("s3://bucket/data/file.parquet").expect("valid url");

        let relative = RemoteDirConnector::relative_path(&source_url, &file_url);
        assert_eq!(relative, "file.parquet");
    }

    #[test]
    fn test_validate_args_copy_true() {
        let func = RemoteDirConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "true".to_string());
        // Note: full validation including copy is done via validate_connector_args
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_copy_false() {
        let func = RemoteDirConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "false".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_connector_args_copy_invalid() {
        use crate::source::connector::validate_connector_args;
        let func = RemoteDirConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "invalid".to_string());

        let result = validate_connector_args(&func, &args);
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("'copy' argument must be 'true' or 'false'"));
    }

    #[test]
    fn test_validate_connector_args_missing_url() {
        use crate::source::connector::validate_connector_args;
        let func = RemoteDirConnector;
        let args = HashMap::new();

        let result = validate_connector_args(&func, &args);
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("requires a 'url' argument"));
    }

    #[test]
    fn test_validate_connector_args_valid() {
        use crate::source::connector::validate_connector_args;
        let func = RemoteDirConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        assert!(validate_connector_args(&func, &args).is_ok());
    }
}
