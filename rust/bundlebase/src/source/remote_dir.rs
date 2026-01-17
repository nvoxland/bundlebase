//! Built-in "remote_dir" source function.
//!
//! Lists files from a directory URL using the IO registry to support
//! any URL scheme (file, s3, gs, azure, ftp, sftp, tar, etc.).

use super::source_function::{
    ArgSpec, AttachedFileInfo, DiscoveredLocation, FetchAction, SourceFunction, SyncMode,
};
use super::source_utils::{self, MaterializeResult};
use crate::io::plugin::ftp::FtpFile;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadWriteDir;
use crate::io::plugin::sftp::{parse_sftp_url, SftpClient};
use crate::io::{io_registry, IOReadFile};
use crate::{BundleConfig, BundlebaseError};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Built-in "remote_dir" source function.
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
/// - `mode` (optional): Sync mode for fetch:
///   - "add" (default): Only attach new files
///   - "update": Add new files and replace changed files
///   - "sync": Add new, replace changed, and remove files no longer at source
pub struct RemoteDirFunction;

#[async_trait]
impl SourceFunction for RemoteDirFunction {
    fn name(&self) -> &str {
        "remote_dir"
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
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
            ArgSpec {
                name: "mode",
                description: "Sync mode: 'add' (default), 'update', or 'sync'",
                required: false,
                default: Some("add"),
            },
        ]
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        self.default_validate_args(args)?;
        // Validate URL is parseable
        source_utils::require_url(args, self.name())?;
        // Validate mode if provided
        if let Some(mode) = args.get("mode") {
            SyncMode::from_arg(mode)?;
        }
        Ok(())
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        let base_url = source_utils::require_url(args, self.name())?;
        let patterns = source_utils::get_patterns(args)?;

        // Use IORegistry to create lister for any URL scheme
        let lister = io_registry()
            .create_lister(&base_url, config.clone())
            .await?;
        let all_files = lister.list_files().await?;

        // Filter files by pattern and already-attached status
        // Use relative path as source_location (relative to base_url)
        let locations: Vec<DiscoveredLocation> = all_files
            .into_iter()
            .filter_map(|file| {
                let relative_path = Self::relative_path(&base_url, &file.url);
                // Check pattern match
                if !patterns.iter().any(|pattern| pattern.matches(&relative_path)) {
                    return None;
                }
                // Check if already attached (by relative path)
                if attached_locations.contains(&relative_path) {
                    return None;
                }
                Some(DiscoveredLocation {
                    url: file.url,
                    source_location: relative_path,
                })
            })
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
        let should_copy = source_utils::should_copy(args);
        let key_path = args.get("key_path").map(|s| s.as_str());

        // Delegate to internal method that handles special protocols
        self.materialize_url(&location.url, should_copy, key_path, data_dir, config)
            .await
    }

    async fn fetch_with_mode(
        &self,
        args: &HashMap<String, String>,
        attached_files: &HashMap<String, AttachedFileInfo>,
        data_dir: &dyn IOReadWriteDir,
        config: Arc<BundleConfig>,
        mode: SyncMode,
    ) -> Result<Vec<FetchAction>, BundlebaseError> {
        let base_url = source_utils::require_url(args, self.name())?;
        let patterns = source_utils::get_patterns(args)?;

        // List all files from the remote directory
        let lister = io_registry()
            .create_lister(&base_url, config.clone())
            .await?;
        let all_files = lister.list_files().await?;

        // Filter files by pattern and convert to DiscoveredLocation
        // Use relative path as source_location (relative to base_url)
        let discovered: Vec<DiscoveredLocation> = all_files
            .into_iter()
            .filter_map(|file| {
                let relative_path = Self::relative_path(&base_url, &file.url);
                // Check pattern match
                if !patterns.iter().any(|pattern| pattern.matches(&relative_path)) {
                    return None;
                }
                Some(DiscoveredLocation {
                    url: file.url,
                    source_location: relative_path,
                })
            })
            .collect();

        // Use shared sync logic
        let config_for_version = config.clone();
        let should_copy = source_utils::should_copy(args);
        let key_path = args.get("key_path").cloned();
        let config_for_materialize = config.clone();

        source_utils::process_sync_mode(
            discovered,
            attached_files,
            data_dir,
            mode,
            |url| {
                let cfg = config_for_version.clone();
                Box::pin(async move { Self::read_remote_version(url, &cfg).await })
            },
            |loc| {
                let cfg = config_for_materialize.clone();
                let kp = key_path.clone();
                async move {
                    Self::materialize_url_static(
                        &loc.url,
                        should_copy,
                        kp.as_deref(),
                        data_dir,
                        &cfg,
                    )
                    .await
                }
            },
        )
        .await
    }
}

impl RemoteDirFunction {
    /// Read the version string from a remote URL.
    ///
    /// Uses IOFile to get version (ETag/S3 version/mtime hash) from the remote file.
    async fn read_remote_version(url: &Url, config: &Arc<BundleConfig>) -> Result<String, BundlebaseError> {
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

    /// Materialize a file to the data directory (static version for use in closures).
    ///
    /// Handles special protocols (SFTP, FTP) that require custom download logic,
    /// and delegates to standard utilities for other schemes.
    /// Returns MaterializeResult containing the file and its SHA256 hash.
    async fn materialize_url_static(
        url: &Url,
        should_copy: bool,
        key_path: Option<&str>,
        data_dir: &dyn IOReadWriteDir,
        config: &Arc<BundleConfig>,
    ) -> Result<MaterializeResult, BundlebaseError> {
        if !should_copy {
            // For non-copied files, compute the hash by streaming
            let file: Box<dyn IOReadFile> = Box::new(ObjectStoreFile::from_url(url, config.clone())?);
            let hash = file.compute_hash().await?;
            return Ok(MaterializeResult { file, hash });
        }

        let scheme = url.scheme();

        // Handle special protocols that need custom download logic
        match scheme {
            "sftp" => Self::download_sftp_static(url, key_path, data_dir).await,
            "ftp" => Self::download_ftp_static(url, data_dir).await,
            _ => {
                // Use standard materialization for other schemes
                source_utils::materialize_url(url, true, data_dir, config).await
            }
        }
    }

    /// Materialize a file to the data directory.
    ///
    /// Handles special protocols (SFTP, FTP) that require custom download logic,
    /// and delegates to standard utilities for other schemes.
    /// Returns MaterializeResult containing the file and its SHA256 hash.
    async fn materialize_url(
        &self,
        url: &Url,
        should_copy: bool,
        key_path: Option<&str>,
        data_dir: &dyn IOReadWriteDir,
        config: &Arc<BundleConfig>,
    ) -> Result<MaterializeResult, BundlebaseError> {
        Self::materialize_url_static(url, should_copy, key_path, data_dir, config).await
    }

    /// Download a file via SFTP (static version).
    /// Returns MaterializeResult containing the file and its SHA256 hash.
    async fn download_sftp_static(
        url: &Url,
        key_path: Option<&str>,
        data_dir: &dyn IOReadWriteDir,
    ) -> Result<MaterializeResult, BundlebaseError> {
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
        let data = sftp.read_file(&remote_path).await?;
        sftp.close().await?;

        // Extract filename and save to data_dir
        let filename = std::path::Path::new(&remote_path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "data".to_string());

        let result = source_utils::download_to_data_dir(data, &filename, data_dir).await?;
        Ok(MaterializeResult {
            file: result.file,
            hash: result.hash,
        })
    }

    /// Download a file via FTP.
    /// Returns MaterializeResult containing the file and its SHA256 hash.
    async fn download_ftp_static(url: &Url, data_dir: &dyn IOReadWriteDir) -> Result<MaterializeResult, BundlebaseError> {
        let ftp_file = FtpFile::from_url(url)?;
        let data = ftp_file.read_bytes().await?.ok_or_else(|| {
            BundlebaseError::from(format!("FTP file not found: {}", url))
        })?;

        let filename = source_utils::filename_from_url(url);
        let result = source_utils::download_to_data_dir(data, &filename, data_dir).await?;
        Ok(MaterializeResult {
            file: result.file,
            hash: result.hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        let func = RemoteDirFunction;
        assert_eq!(func.name(), "remote_dir");
    }

    #[test]
    fn test_arg_specs() {
        let func = RemoteDirFunction;
        let specs = func.arg_specs();
        assert_eq!(specs.len(), 5);
        assert!(specs.iter().any(|s| s.name == "url" && s.required));
        assert!(specs.iter().any(|s| s.name == "patterns" && !s.required));
        assert!(specs.iter().any(|s| s.name == "copy" && !s.required));
        assert!(specs.iter().any(|s| s.name == "key_path" && !s.required));
        assert!(specs.iter().any(|s| s.name == "mode" && !s.required));
    }

    #[test]
    fn test_validate_args_with_url() {
        let func = RemoteDirFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_missing_url() {
        let func = RemoteDirFunction;
        let args = HashMap::new();

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires a 'url' argument"));
    }

    #[test]
    fn test_validate_args_invalid_url() {
        let func = RemoteDirFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "not-a-valid-url".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid URL"));
    }

    #[test]
    fn test_relative_path() {
        let source_url = Url::parse("s3://bucket/data/").unwrap();
        let file_url = Url::parse("s3://bucket/data/subdir/file.parquet").unwrap();

        let relative = RemoteDirFunction::relative_path(&source_url, &file_url);
        assert_eq!(relative, "subdir/file.parquet");
    }

    #[test]
    fn test_relative_path_root() {
        let source_url = Url::parse("s3://bucket/data/").unwrap();
        let file_url = Url::parse("s3://bucket/data/file.parquet").unwrap();

        let relative = RemoteDirFunction::relative_path(&source_url, &file_url);
        assert_eq!(relative, "file.parquet");
    }

    #[test]
    fn test_validate_args_copy_true() {
        let func = RemoteDirFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "true".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_copy_false() {
        let func = RemoteDirFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "false".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_copy_invalid() {
        let func = RemoteDirFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "invalid".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("'copy' argument must be 'true' or 'false'"));
    }
}
