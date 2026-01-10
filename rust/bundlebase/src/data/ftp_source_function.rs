//! FTP source function for listing files from remote FTP directories.

use crate::data::source_function::SourceFunction;
use crate::io::{parse_ftp_url, FtpClient, ObjectStoreFile};
use crate::BundlebaseError;
use crate::BundleConfig;
use async_trait::async_trait;
use glob::Pattern;
use std::collections::HashMap;
use std::sync::Arc;

/// Built-in "ftp_directory" source function.
///
/// Lists files from a remote directory via FTP.
///
/// Arguments:
/// - `url` (required): FTP URL (e.g., "ftp://user:pass@ftp.example.com:21/path/to/data")
/// - `patterns` (optional): Comma-separated glob patterns (defaults to "**/*")
///
/// URL format supports:
/// - Anonymous FTP: `ftp://ftp.example.com/pub/data`
/// - Authenticated: `ftp://user:password@ftp.example.com/data`
/// - Custom port: `ftp://ftp.example.com:2121/data`
///
/// Note: Unlike `data_directory`, files are always copied into the bundle's data directory
/// since remote FTP files cannot be directly referenced at query time.
pub struct FtpDirectoryFunction;

#[async_trait]
impl SourceFunction for FtpDirectoryFunction {
    fn name(&self) -> &str {
        "ftp_directory"
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // Validate 'url' is present and parseable as ftp://
        if !args.contains_key("url") {
            return Err(format!(
                "Function '{}' requires a 'url' argument",
                self.name()
            )
            .into());
        }

        let url_str = args.get("url").expect("checked above");
        let url = url::Url::parse(url_str).map_err(|e| {
            BundlebaseError::from(format!("Invalid URL '{}': {}", url_str, e))
        })?;

        // Validate it's an FTP URL and can be parsed
        parse_ftp_url(&url)?;

        Ok(())
    }

    async fn list_files(
        &self,
        args: &HashMap<String, String>,
        _config: Arc<BundleConfig>,
    ) -> Result<Vec<ObjectStoreFile>, BundlebaseError> {
        // Get URL from args
        let url_str = args.get("url").ok_or_else(|| {
            BundlebaseError::from(format!(
                "Function '{}' requires a 'url' argument",
                self.name()
            ))
        })?;
        let url = url::Url::parse(url_str)?;
        let (user, password, host, port, remote_path) = parse_ftp_url(&url)?;

        // Get patterns from args, defaulting to "**/*"
        let patterns_str = args
            .get("patterns")
            .map(|s| s.as_str())
            .unwrap_or("**/*");
        let patterns: Vec<&str> = patterns_str.split(',').map(|s| s.trim()).collect();

        // Compile glob patterns
        let compiled_patterns: Vec<Pattern> = patterns
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();

        // Connect to FTP
        let mut ftp = FtpClient::connect(&host, port, &user, &password).await?;

        // List all files recursively
        let all_files = ftp.list_files_recursive(&remote_path).await?;

        // Close connection (we only need the listing; files will be downloaded during refresh)
        ftp.close().await?;

        // Filter files by pattern and convert to ObjectStoreFile
        // We use the original ftp:// URL as the file location
        let matching_files: Vec<ObjectStoreFile> = all_files
            .into_iter()
            .filter(|file| {
                let relative_path = Self::relative_path(&remote_path, &file.path);
                compiled_patterns
                    .iter()
                    .any(|pattern| pattern.matches(&relative_path))
            })
            .filter_map(|file| {
                // Construct the full FTP URL for this file
                let file_url = if password.is_empty() {
                    if user == "anonymous" {
                        format!("ftp://{}:{}{}", host, port, file.path)
                    } else {
                        format!("ftp://{}@{}:{}{}", user, host, port, file.path)
                    }
                } else {
                    format!("ftp://{}:{}@{}:{}{}", user, password, host, port, file.path)
                };

                match url::Url::parse(&file_url) {
                    Ok(url) => {
                        // Create a placeholder ObjectStoreFile
                        // This file can't be read directly - it will be downloaded during refresh
                        // We use the memory store with a temporary path
                        let _memory_url = url::Url::parse(&format!(
                            "memory:///ftp-pending/{}",
                            file.path.trim_start_matches('/')
                        )).ok()?;

                        ObjectStoreFile::new(
                            &url,
                            crate::io::get_memory_store(),
                            &object_store::path::Path::from(file.path.trim_start_matches('/')),
                        ).ok()
                    }
                    Err(_) => None,
                }
            })
            .collect();

        Ok(matching_files)
    }
}

impl FtpDirectoryFunction {
    /// Get the relative path of a file compared to the source directory.
    fn relative_path(source_path: &str, file_path: &str) -> String {
        // Normalize paths by removing trailing slashes for comparison
        let source_normalized = source_path.trim_end_matches('/');

        if let Some(stripped) = file_path.strip_prefix(source_normalized) {
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
    fn test_ftp_directory_validate_args_valid() {
        let func = FtpDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "ftp://ftp.example.com/pub/data".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ftp_directory_validate_args_with_auth() {
        let func = FtpDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "ftp://user:pass@ftp.example.com:2121/data".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ftp_directory_validate_args_missing_url() {
        let func = FtpDirectoryFunction;
        let args = HashMap::new();

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires a 'url' argument"));
    }

    #[test]
    fn test_ftp_directory_validate_args_invalid_url_scheme() {
        let func = FtpDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "http://example.com/data".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 'ftp'"));
    }

    #[test]
    fn test_relative_path() {
        assert_eq!(
            FtpDirectoryFunction::relative_path("/data/files", "/data/files/subdir/file.txt"),
            "subdir/file.txt"
        );

        assert_eq!(
            FtpDirectoryFunction::relative_path("/data/files/", "/data/files/file.txt"),
            "file.txt"
        );

        assert_eq!(
            FtpDirectoryFunction::relative_path("/pub", "/pub/nested/deep/file.parquet"),
            "nested/deep/file.parquet"
        );
    }

    #[test]
    fn test_ftp_directory_name() {
        let func = FtpDirectoryFunction;
        assert_eq!(func.name(), "ftp_directory");
    }
}
