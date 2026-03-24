//! Test utilities for the index crate.

use bundlebase_common::{BundlebaseError, ConfigProvider, ConfigKey, Scope};
use bundlebase_io::IOReadWriteDir;
use std::sync::Arc;
use url::Url;

struct EmptyConfigProvider;

impl ConfigProvider for EmptyConfigProvider {
    fn get(&self, _scope: &Scope, _key: &ConfigKey) -> Result<Option<String>, BundlebaseError> {
        Ok(None)
    }
}

fn test_config() -> Arc<dyn ConfigProvider> {
    Arc::new(EmptyConfigProvider)
}

pub fn random_memory_url() -> Url {
    Url::parse(&format!("memory:///{}", rand::random::<u64>())).expect("valid URL")
}

pub fn random_memory_dir() -> Arc<dyn IOReadWriteDir> {
    let url = random_memory_url();
    let store = bundlebase_io::get_memory_store();
    bundlebase_io::writable_dir_with_store(&url, store, &object_store::path::Path::from(url.path()), test_config())
        .expect("memory dir creation should not fail")
}

pub fn random_memory_dir_concrete() -> bundlebase_io::plugin::object_store::ObjectStoreDir {
    bundlebase_io::plugin::object_store::ObjectStoreDir::from_url(&random_memory_url(), test_config())
        .expect("memory dir creation should not fail")
}
