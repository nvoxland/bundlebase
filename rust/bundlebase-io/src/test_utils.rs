//! Test utilities for the IO crate.

use crate::{
    get_memory_store, writable_dir_with_store, BundlebaseError, ConfigProvider, IOReadWriteDir,
};
use std::sync::Arc;
use url::Url;

use parking_lot::RwLock;
use std::collections::HashMap;

/// A configurable ConfigProvider for tests.
/// Supports both empty configs and configs with pre-set key/value pairs.
pub struct TestConfigProvider {
    values: RwLock<HashMap<(String, String), String>>,
}

impl TestConfigProvider {
    pub fn new() -> Self {
        Self {
            values: RwLock::new(HashMap::new()),
        }
    }

    /// Set a value for a scope+key pair.
    pub fn set(&self, scope: &str, key: &str, value: &str) {
        self.values
            .write()
            .insert((scope.to_string(), key.to_string()), value.to_string());
    }
}

impl ConfigProvider for TestConfigProvider {
    fn get_in_scope(
        &self,
        scope: &bundlebase_common::Scope,
        key: &bundlebase_common::ConfigKey,
    ) -> Result<Option<String>, BundlebaseError> {
        // Check for exact scope match, then prefix matches
        let values = self.values.read();
        // Try exact scope
        if let Some(v) = values.get(&(scope.as_str().to_string(), key.key.to_string())) {
            return Ok(Some(v.clone()));
        }
        // Try scope prefix (e.g., "ftp" matches "ftp/host/path")
        if key.scope.matches(scope) {
            if let Some(v) = values.get(&(key.scope.name.to_string(), key.key.to_string())) {
                return Ok(Some(v.clone()));
            }
        }
        Ok(None)
    }
}

/// Create a test config that returns no values.
/// Sufficient for memory:// and file:// backends.
pub fn test_config() -> Arc<dyn ConfigProvider> {
    Arc::new(TestConfigProvider::new())
}

/// Create a test config with pre-set values.
pub fn test_config_with_values(values: &[(&str, &str, &str)]) -> Arc<TestConfigProvider> {
    let config = Arc::new(TestConfigProvider::new());
    for (scope, key, value) in values {
        config.set(scope, key, value);
    }
    config
}

/// Create a random memory URL for testing.
pub fn random_memory_url() -> Url {
    Url::parse(&format!("memory:///{}", rand::random::<u64>())).expect("valid memory URL")
}

/// Create a random memory directory for testing.
pub fn random_memory_dir() -> Arc<dyn IOReadWriteDir> {
    let url = random_memory_url();
    let store = get_memory_store();
    writable_dir_with_store(
        &url,
        store,
        &object_store::path::Path::from(url.path()),
        test_config(),
    )
    .expect("memory dir creation should not fail")
}

/// Create a random memory file for testing.
pub fn random_memory_file(path: &str) -> Box<dyn crate::IOReadWriteFile> {
    random_memory_dir()
        .writable_file(path)
        .expect("writable file creation should not fail")
}

/// Create a concrete ObjectStoreDir for tests that need the specific type.
pub fn random_memory_dir_concrete() -> crate::plugin::object_store::ObjectStoreDir {
    crate::plugin::object_store::ObjectStoreDir::from_url(&random_memory_url(), test_config())
        .expect("memory dir creation should not fail")
}
