//! Test utilities for the connector crate.

use bundlebase_common::{BundlebaseError, ConfigKey, ConfigProvider, Scope};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// A configurable ConfigProvider for tests.
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
    fn get(&self, scope: &Scope, key: &ConfigKey) -> Result<Option<String>, BundlebaseError> {
        let values = self.values.read();
        if let Some(v) = values.get(&(scope.as_str().to_string(), key.key.to_string())) {
            return Ok(Some(v.clone()));
        }
        if key.scope.matches(scope) {
            if let Some(v) = values.get(&(key.scope.name.to_string(), key.key.to_string())) {
                return Ok(Some(v.clone()));
            }
        }
        // Check key defaults
        if key.scope.matches(scope) {
            if let Some(v) = key.resolve_default() {
                return Ok(Some(v));
            }
        }
        Ok(None)
    }
}

/// Create a test config that returns no values (except key defaults).
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
