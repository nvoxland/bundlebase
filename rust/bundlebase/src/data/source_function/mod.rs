//! Source function system for file discovery.
//!
//! Source functions define how files are discovered and listed.
//! Different implementations can provide different strategies (e.g., directory listing,
//! S3 inventory, database queries, etc.).

mod data_directory;

use crate::io::IOFile;
use crate::{BundlebaseError, BundleConfig};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

pub use data_directory::DataDirectoryFunction;

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
    ) -> Result<Vec<IOFile>, BundlebaseError>;
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

        // Register built-in functions
        // DataDirectoryFunction now handles all URL schemes via IORegistry
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
}
