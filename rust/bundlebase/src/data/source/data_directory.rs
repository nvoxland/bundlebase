//! Built-in "data_directory" source function.
//!
//! Lists files from a directory URL using the IO registry to support
//! any URL scheme (file, s3, gs, azure, ftp, sftp, tar, etc.).

use super::source_function::SourceFunction;
use crate::io::{io_registry, IOFile};
use crate::{BundlebaseError, BundleConfig};
use async_trait::async_trait;
use glob::Pattern;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

/// Built-in "data_directory" source function.
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
pub struct DataDirectoryFunction;

#[async_trait]
impl SourceFunction for DataDirectoryFunction {
    fn name(&self) -> &str {
        "data_directory"
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // data_directory requires a "url" argument
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

    async fn list_files(
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

        // Compile glob patterns
        let compiled_patterns: Vec<Pattern> = patterns
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();

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
}

impl DataDirectoryFunction {
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
    fn test_data_directory_validate_args_with_url() {
        let func = DataDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_data_directory_validate_args_missing_url() {
        let func = DataDirectoryFunction;
        let args = HashMap::new();

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires a 'url' argument"));
    }

    #[test]
    fn test_data_directory_validate_args_invalid_url() {
        let func = DataDirectoryFunction;
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

        let relative = DataDirectoryFunction::relative_path(&source_url, &file_url);
        assert_eq!(relative, "subdir/file.parquet");
    }

    #[test]
    fn test_relative_path_root() {
        let source_url = Url::parse("s3://bucket/data/").unwrap();
        let file_url = Url::parse("s3://bucket/data/file.parquet").unwrap();

        let relative = DataDirectoryFunction::relative_path(&source_url, &file_url);
        assert_eq!(relative, "file.parquet");
    }

    #[test]
    fn test_data_directory_validate_args_copy_true() {
        let func = DataDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "true".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_data_directory_validate_args_copy_false() {
        let func = DataDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("copy".to_string(), "false".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_data_directory_validate_args_copy_invalid() {
        let func = DataDirectoryFunction;
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
