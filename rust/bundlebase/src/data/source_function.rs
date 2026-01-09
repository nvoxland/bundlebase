use crate::io::{ObjectStoreDir, ObjectStoreFile};
use crate::{BundlebaseError, BundleConfig};
use async_trait::async_trait;
use glob::Pattern;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

/// Trait for source function implementations.
///
/// Source functions define how files are discovered and listed from a source URL.
/// Different implementations can provide different strategies (e.g., directory listing,
/// S3 inventory, database queries, etc.).
#[async_trait]
pub trait SourceFunction: Send + Sync {
    /// Name of this source function
    fn name(&self) -> &str;

    /// Validate arguments for this function
    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError>;

    /// List files from the source URL with function-specific logic
    async fn list_files(
        &self,
        url: &Url,
        patterns: &[String],
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
/// Takes no arguments.
pub struct DataDirectoryFunction;

#[async_trait]
impl SourceFunction for DataDirectoryFunction {
    fn name(&self) -> &str {
        "data_directory"
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // data_directory takes no arguments
        if !args.is_empty() {
            return Err(format!(
                "Function '{}' takes no arguments, but {} were provided",
                self.name(),
                args.len()
            )
            .into());
        }
        Ok(())
    }

    async fn list_files(
        &self,
        url: &Url,
        patterns: &[String],
        _args: &HashMap<String, String>,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<ObjectStoreFile>, BundlebaseError> {
        // List all files from the directory
        let dir = ObjectStoreDir::from_url(url, config)?;
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
                let relative_path = Self::relative_path(url, file.url());
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
    fn test_data_directory_validate_args_empty() {
        let func = DataDirectoryFunction;
        let args = HashMap::new();
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_data_directory_validate_args_non_empty() {
        let func = DataDirectoryFunction;
        let mut args = HashMap::new();
        args.insert("invalid".to_string(), "arg".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("takes no arguments"));
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
