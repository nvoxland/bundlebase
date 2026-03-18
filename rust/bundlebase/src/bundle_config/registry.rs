use std::sync::OnceLock;
use super::{ConfigKey, ConfigScope};

/// Central registry of all known config scopes and keys.
pub struct ConfigRegistry {
    scopes: Vec<ConfigScope>,
    keys: Vec<ConfigKey>,
}

impl ConfigRegistry {
    fn new() -> Self {
        Self { scopes: Vec::new(), keys: Vec::new() }
    }

    pub fn register_scopes(&mut self, scopes: &[ConfigScope]) {
        self.scopes.extend_from_slice(scopes);
    }

    pub fn register_keys(&mut self, keys: &[ConfigKey]) {
        self.keys.extend_from_slice(keys);
    }

    pub fn scopes(&self) -> &[ConfigScope] {
        &self.scopes
    }

    pub fn keys(&self) -> &[ConfigKey] {
        &self.keys
    }
}

static CONFIG_REGISTRY: OnceLock<ConfigRegistry> = OnceLock::new();

/// Get the global config registry instance.
/// This registry has all built-in scopes and keys already registered.
pub fn config_registry() -> &'static ConfigRegistry {
    CONFIG_REGISTRY.get_or_init(|| {
        let mut registry = ConfigRegistry::new();
        register_builtin_configs(&mut registry);
        registry
    })
}

fn register_builtin_configs(registry: &mut ConfigRegistry) {
    use super::system;

    registry.register_scopes(system::system_scopes());
    registry.register_keys(system::system_keys());

    use crate::io::plugin;
    registry.register_scopes(plugin::object_store::object_store_scopes());
    registry.register_keys(plugin::object_store::s3_keys());
    registry.register_keys(plugin::object_store::gcs_keys());
    registry.register_keys(plugin::object_store::azure_keys());

    registry.register_scopes(plugin::ftp::ftp_scopes());
    registry.register_keys(plugin::ftp::ftp_keys());

    registry.register_scopes(plugin::sftp::sftp_scopes());
    registry.register_keys(plugin::sftp::sftp_keys());

    use crate::connector::plugin::kaggle;
    registry.register_scopes(kaggle::scopes());
    registry.register_keys(kaggle::configs());
}
