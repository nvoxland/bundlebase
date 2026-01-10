use crate::io::{ObjectStoreDir, ObjectStoreFile};
use crate::{BundlebaseError, BundleConfig};
use async_trait::async_trait;
use glob::Pattern;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

/// Trait for source function implementations.
///
/// Source functions define how files are discovered and listed.
/// Different implementations can provide different strategies (e.g., directory listing,
/// S3 inventory, database queries, etc.).
///
/// Each function defines its own required and optional arguments. For example,
/// "data_directory" requires:
/// - "url": Directory URL to list
/// - "patterns": Comma-separated glob patterns (optional, defaults to "**/*")
#[async_trait]
pub trait SourceFunction: Send + Sync {
    /// Name of this source function
    fn name(&self) -> &str;

    /// Validate arguments for this function.
    /// Should check for required arguments and validate their values.
    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError>;

    /// List files using function-specific logic.
    /// Arguments contain all configuration needed by the function.
    async fn list_files(
        &self,
        args: &HashMap<String, String>,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<ObjectStoreFile>, BundlebaseError>;
}

/// Registry for source functions.
///
/// Manages available source functions and provides lookup by name.
/// Built-in functions are automatically registered on construction.
pub struct SourceFunctionRegistry {
    functions: HashMap<String, Arc<dyn SourceFunction>>,
}

impl SourceFunctionRegistry {
    /// Create a new registry with built-in functions registered.
    pub fn new() -> Self {
        let mut registry = Self {
            functions: HashMap::new(),
        };

        // Register built-in "data_directory" function
        registry.register(Arc::new(DataDirectoryFunction));

        registry
    }

    /// Register a source function.
    pub fn register(&mut self, func: Arc<dyn SourceFunction>) {
        self.functions.insert(func.name().to_string(), func);
    }

    /// Get a source function by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn SourceFunction>> {
        self.functions.get(name).cloned()
    }

    /// Get all registered function names.
    pub fn function_names(&self) -> Vec<String> {
        self.functions.keys().cloned().collect()
    }
}

impl Default for SourceFunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in "data_directory" source function.
///
/// Lists files from a directory URL using standard object store listing.
/// Supports glob patterns for filtering files.
///
/// Arguments:
/// - `url` (required): The directory URL to list (e.g., "s3://bucket/data/")
/// - `patterns` (optional): Comma-separated glob patterns (e.g., "**/*.parquet,**/*.csv")
///   Defaults to "**/*" (all files)
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

        Ok(())
    }

    async fn list_files(
        &self,
        args: &HashMap<String, String>,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<ObjectStoreFile>, BundlebaseError> {
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

        // List all files from the directory
        let dir = ObjectStoreDir::from_url(&url, config)?;
        let all_files = dir.list_files().await?;

        // Compile glob patterns
        let compiled_patterns: Vec<Pattern> = patterns
            .iter()
            .filter_map(|p| Pattern::new(p).ok())
            .collect();

        // Filter files by pattern
        let matching_files: Vec<ObjectStoreFile> = all_files
            .into_iter()
            .filter(|file| {
                let relative_path = Self::relative_path(&url, file.url());
                compiled_patterns
                    .iter()
                    .any(|pattern| pattern.matches(&relative_path))
            })
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
    fn test_registry_new() {
        let registry = SourceFunctionRegistry::new();
        assert!(registry.get("data_directory").is_some());
    }

    #[test]
    fn test_registry_get() {
        let registry = SourceFunctionRegistry::new();
        let func = registry.get("data_directory").unwrap();
        assert_eq!(func.name(), "data_directory");
    }

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
}
