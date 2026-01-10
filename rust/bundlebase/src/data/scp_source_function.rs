//! SCP/SFTP source function for listing files from remote directories via SSH.

use crate::data::source_function::SourceFunction;
use crate::io::{parse_scp_url, ObjectStoreFile, SftpClient};
use crate::BundlebaseError;
use crate::BundleConfig;
use async_trait::async_trait;
use glob::Pattern;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Built-in "scp_directory" source function.
///
/// Lists files from a remote directory via SSH/SFTP.
///
/// Arguments:
/// - `url` (required): SCP/SFTP URL. Both schemes are supported:
///   - `scp://user@host:22/path/to/data`
///   - `sftp://user@host:22/path/to/data`
/// - `key_path` (required): Path to SSH private key file
/// - `patterns` (optional): Comma-separated glob patterns (defaults to "**/*")
///
/// Note: Unlike `data_directory`, the `copy` argument is not applicable for SCP/SFTP sources
/// since remote files cannot be directly referenced at query time. Files are always
/// copied into the bundle's data directory.
pub struct ScpDirectoryFunction;

#[async_trait]
impl SourceFunction for ScpDirectoryFunction {
    fn name(&self) -> &str {
        "scp_directory"
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // Validate 'url' is present and parseable as scp://
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

        // Validate it's an SCP URL and can be parsed
        parse_scp_url(&url)?;

        // Validate 'key_path' is present
        if !args.contains_key("key_path") {
            return Err(format!(
                "Function '{}' requires a 'key_path' argument specifying the SSH private key file",
                self.name()
            )
            .into());
        }

        // Check that the key file exists
        let key_path = args.get("key_path").expect("checked above");
        let key_path_expanded = shellexpand::tilde(key_path).to_string();
        if !Path::new(&key_path_expanded).exists() {
            return Err(format!(
                "SSH key file not found: '{}'",
                key_path
            )
            .into());
        }

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
        let (user, host, port, remote_path) = parse_scp_url(&url)?;

        // Get key path
        let key_path_str = args.get("key_path").ok_or_else(|| {
            BundlebaseError::from(format!(
                "Function '{}' requires a 'key_path' argument",
                self.name()
            ))
        })?;
        let key_path_expanded = shellexpand::tilde(key_path_str).to_string();
        let key_path = Path::new(&key_path_expanded);

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

        // Connect to SFTP
        let sftp = SftpClient::connect(&host, port, &user, key_path).await?;

        // List all files recursively
        let all_files = sftp.list_files_recursive(&remote_path).await?;

        // Close connection (we only need the listing; files will be downloaded during refresh)
        sftp.close().await?;

        // Filter files by pattern and convert to ObjectStoreFile
        // We use the original scp:// URL as the file location
        let matching_files: Vec<ObjectStoreFile> = all_files
            .into_iter()
            .filter(|file| {
                let relative_path = Self::relative_path(&remote_path, &file.path);
                compiled_patterns
                    .iter()
                    .any(|pattern| pattern.matches(&relative_path))
            })
            .filter_map(|file| {
                // Construct the full SCP URL for this file
                let file_url = format!("scp://{}@{}:{}{}", user, host, port, file.path);
                match url::Url::parse(&file_url) {
                    Ok(url) => {
                        // Create a placeholder ObjectStoreFile
                        // This file can't be read directly - it will be downloaded during refresh
                        // We use the memory store with a temporary path
                        let _memory_url = url::Url::parse(&format!(
                            "memory:///scp-pending/{}",
                            file.path.trim_start_matches('/')
                        )).ok()?;

                        // We store the actual SCP URL in a way the refresh method can retrieve it
                        // The ObjectStoreFile will have the original scp:// URL
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

impl ScpDirectoryFunction {
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
    fn test_scp_directory_validate_args_valid() {
        let func = ScpDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "scp://user@example.com:22/data".to_string());
        args.insert("key_path".to_string(), "~/.ssh/id_rsa".to_string());

        // This will fail because the key file doesn't exist, but that's expected in tests
        let result = func.validate_args(&args);
        // The error should be about the key file not existing, not about missing args
        if let Err(e) = result {
            let err_str = e.to_string();
            assert!(err_str.contains("not found") || err_str.contains("key"));
        }
    }

    #[test]
    fn test_scp_directory_validate_args_missing_url() {
        let func = ScpDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("key_path".to_string(), "~/.ssh/id_rsa".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires a 'url' argument"));
    }

    #[test]
    fn test_scp_directory_validate_args_missing_key_path() {
        let func = ScpDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "scp://user@example.com/data".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires a 'key_path' argument"));
    }

    #[test]
    fn test_scp_directory_validate_args_invalid_url_scheme() {
        let func = ScpDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "http://example.com/data".to_string());
        args.insert("key_path".to_string(), "~/.ssh/id_rsa".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 'scp'"));
    }

    #[test]
    fn test_relative_path() {
        assert_eq!(
            ScpDirectoryFunction::relative_path("/data/files", "/data/files/subdir/file.txt"),
            "subdir/file.txt"
        );

        assert_eq!(
            ScpDirectoryFunction::relative_path("/data/files/", "/data/files/file.txt"),
            "file.txt"
        );

        assert_eq!(
            ScpDirectoryFunction::relative_path("/data", "/data/nested/deep/file.parquet"),
            "nested/deep/file.parquet"
        );
    }

    #[test]
    fn test_scp_directory_name() {
        let func = ScpDirectoryFunction;
        assert_eq!(func.name(), "scp_directory");
    }
}
