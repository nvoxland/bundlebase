//! Built-in "remote_dir" source function.
//!
//! Lists files from a directory URL using the IO registry to support
//! any URL scheme (file, s3, gs, azure, ftp, sftp, tar, etc.).

use super::source_function::{MaterializedData, SourceFunction};
use crate::data::ObjectId;
use crate::io::{io_registry, parse_scp_url, FtpFile, IODir, IOFile, IOReader, IOWriter, SftpClient};
use crate::{BundlebaseError, BundleConfig};
use async_trait::async_trait;
use glob::Pattern;
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
pub struct RemoteDirFunction;

#[async_trait]
impl SourceFunction for RemoteDirFunction {
    fn name(&self) -> &str {
        "remote_dir"
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // remote_dir requires a "url" argument
        if !args.contains_key("url") {
            return Err(format!(
                "Function '{}' requires a 'url' argument",
                self.name()
            )
            .into());
        }

        // Validate the URL is parseable
        let url_str = args.get("url").expect("checked above");
        Url::parse(url_str).map_err(|e| {
            BundlebaseError::from(format!("Invalid URL '{}': {}", url_str, e))
        })?;

        // Validate "copy" argument if present (must be "true" or "false")
        if let Some(copy_val) = args.get("copy") {
            if copy_val != "true" && copy_val != "false" {
                return Err(format!(
                    "Function '{}': 'copy' argument must be 'true' or 'false', got '{}'",
                    self.name(),
                    copy_val
                )
                .into());
            }
        }

        Ok(())
    }

    async fn refresh(
        &self,
        args: &HashMap<String, String>,
        attached_locations: HashSet<String>,
        data_dir: &IODir,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<MaterializedData>, BundlebaseError> {
        // List all files and filter out already-attached ones
        let all_files = self.list_files_internal(args, config.clone()).await?;
        let pending: Vec<_> = all_files
            .into_iter()
            .filter(|f| !attached_locations.contains(f.url().as_str()))
            .collect();

        // Check if we should copy files (default: true)
        let should_copy = args.get("copy").map(|s| s != "false").unwrap_or(true);
        let key_path = args.get("key_path").cloned();

        let mut results = Vec::new();
        for file in pending {
            let original_url = file.url().to_string();

            // Materialize the file (download/copy as needed)
            let attach_location = self
                .materialize_file(&file, should_copy, key_path.as_deref(), data_dir, config.clone())
                .await?;

            results.push(MaterializedData {
                attach_location,
                source_location: original_url,
            });
        }

        Ok(results)
    }
}

impl RemoteDirFunction {
    /// List files from the directory, applying glob patterns.
    async fn list_files_internal(
        &self,
        args: &HashMap<String, String>,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<IOFile>, BundlebaseError> {
        // Get URL from args
        let url_str = args.get("url").ok_or_else(|| {
            BundlebaseError::from(format!(
                "Function '{}' requires a 'url' argument",
                self.name()
            ))
        })?;
        let url = Url::parse(url_str)?;

        // Get patterns from args, defaulting to "**/*"
        let patterns_str = args
            .get("patterns")
            .map(|s| s.as_str())
            .unwrap_or("**/*");
        let patterns: Vec<&str> = patterns_str.split(',').map(|s| s.trim()).collect();

        // Use IORegistry to create lister for any URL scheme
        let lister = io_registry().create_lister(&url, config.clone()).await?;
        let all_files = lister.list_files().await?;

        // Compile glob patterns - fail on invalid patterns instead of silently ignoring
        let compiled_patterns: Vec<Pattern> = patterns
            .iter()
            .map(|p| {
                Pattern::new(p).map_err(|e| {
                    BundlebaseError::from(format!("Invalid glob pattern '{}': {}", p, e))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Filter files by pattern and convert FileInfo to IOFile
        let matching_files: Vec<IOFile> = all_files
            .into_iter()
            .filter(|file| {
                let relative_path = Self::relative_path(&url, &file.url);
                compiled_patterns
                    .iter()
                    .any(|pattern| pattern.matches(&relative_path))
            })
            .filter_map(|file| IOFile::from_url(&file.url, config.clone()).ok())
            .collect();

        Ok(matching_files)
    }

    /// Materialize a file to the data directory (download/copy as needed).
    /// Returns the location where the file was materialized.
    async fn materialize_file(
        &self,
        file: &IOFile,
        should_copy: bool,
        key_path: Option<&str>,
        data_dir: &IODir,
        config: Arc<BundleConfig>,
    ) -> Result<String, BundlebaseError> {
        let original_url = file.url().to_string();
        let parsed_url = file.url().clone();
        let scheme = parsed_url.scheme();

        // Remote files (SCP/SFTP/FTP) must always be copied
        if scheme == "scp" || scheme == "sftp" {
            // Download file via SFTP
            let (user, host, port, remote_path) = parse_scp_url(&parsed_url)?;
            let key_path_str = key_path.ok_or_else(|| {
                BundlebaseError::from(
                    "SCP/SFTP source requires 'key_path' argument for downloading files",
                )
            })?;
            let key_path_expanded = shellexpand::tilde(key_path_str).to_string();

            let sftp =
                SftpClient::connect(&host, port, &user, std::path::Path::new(&key_path_expanded))
                    .await?;
            let data = sftp.read_file(&remote_path).await?;
            sftp.close().await?;

            // Generate unique filename in data_dir
            let filename = std::path::Path::new(&remote_path)
                .file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "data".to_string());
            let block_id = ObjectId::generate();
            let target_name = format!("{}_{}", block_id, filename);
            let target_file = data_dir.io_file(&target_name)?;
            target_file.write(data).await?;

            Ok(target_file.url().to_string())
        } else if scheme == "ftp" {
            // Download file via FTP
            let ftp_file = FtpFile::from_url(&parsed_url)?;
            let data = ftp_file.read_bytes().await?.ok_or_else(|| {
                BundlebaseError::from(format!("FTP file not found: {}", original_url))
            })?;

            // Generate unique filename in data_dir
            let filename = parsed_url
                .path_segments()
                .and_then(|s| s.last())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "data".to_string());
            let block_id = ObjectId::generate();
            let target_name = format!("{}_{}", block_id, filename);
            let target_file = data_dir.io_file(&target_name)?;
            target_file.write(data).await?;

            Ok(target_file.url().to_string())
        } else if should_copy {
            // Copy file to data_dir (local/cloud files)
            let source_file = IOFile::from_url(&parsed_url, config)?;
            let data = source_file.read_bytes().await?.ok_or_else(|| {
                BundlebaseError::from(format!("Source file not found: {}", original_url))
            })?;

            // Generate unique filename in data_dir
            let filename = parsed_url
                .path_segments()
                .and_then(|s| s.last())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "data".to_string());
            let block_id = ObjectId::generate();
            let target_name = format!("{}_{}", block_id, filename);
            let target_file = data_dir.io_file(&target_name)?;
            target_file.write(data).await?;

            Ok(target_file.url().to_string())
        } else {
            // Reference file at original URL
            Ok(original_url)
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_dir_validate_args_with_url() {
        let func = RemoteDirFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_remote_dir_validate_args_missing_url() {
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
    fn test_remote_dir_validate_args_invalid_url() {
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
    fn test_remote_dir_validate_args_copy_true() {
        let func = RemoteDirFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "true".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_remote_dir_validate_args_copy_false() {
        let func = RemoteDirFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "false".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_remote_dir_validate_args_copy_invalid() {
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
