//! Source function system for data discovery and materialization.
//!
//! Source functions define how data is discovered and materialized into files.
//! Different implementations can provide different strategies (e.g., directory listing,
//! database queries, API pagination, etc.).

use super::remote_dir::RemoteDirFunction;
use crate::io::IODir;
use crate::{BundlebaseError, BundleConfig};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Result of materializing a single data unit from a source.
#[derive(Debug, Clone)]
pub struct MaterializedData {
    /// Location of the materialized file (URL in data_dir or original if not copied)
    pub attach_location: String,
    /// Original source location identifier (file URL, row range, etc.)
    pub source_location: String,
}

/// Trait for source function implementations.
///
/// Source functions define how data is discovered and materialized.
/// Each source function controls:
/// - What "location" means (file URL, row range, API cursor, etc.)
/// - How to materialize data into files
/// - What data to return for attachment
///
/// Each function defines its own required and optional arguments. For example,
/// "remote_dir" requires:
/// - "url": Directory URL to list
/// - "patterns": Comma-separated glob patterns (optional, defaults to "**/*")
#[async_trait]
pub trait SourceFunction: Send + Sync {
    /// Name of this source function
    fn name(&self) -> &str;

    /// Validate arguments for this function.
    /// Should check for required arguments and validate their values.
    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError>;

    /// Refresh the source: find new data and materialize it.
    ///
    /// # Arguments
    /// * `args` - Source configuration
    /// * `attached_locations` - Locations already attached from this source
    /// * `data_dir` - Where to write materialized files
    /// * `config` - Bundle configuration
    ///
    /// # Returns
    /// List of materialized data ready for attachment
    async fn refresh(
        &self,
        args: &HashMap<String, String>,
        attached_locations: HashSet<String>,
        data_dir: &IODir,
        config: Arc<BundleConfig>,
    ) -> Result<Vec<MaterializedData>, BundlebaseError>;
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
        // RemoteDirFunction handles all URL schemes via IORegistry
        registry.register(Arc::new(RemoteDirFunction));

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
        assert!(registry.get("remote_dir").is_some());
    }

    #[test]
    fn test_registry_get() {
        let registry = SourceFunctionRegistry::new();
        let func = registry.get("remote_dir").unwrap();
        assert_eq!(func.name(), "remote_dir");
    }
}
