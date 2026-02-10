//! System scope configuration.
//!
//! The system scope provides bundlebase-level settings like memory limits
//! and catalog naming that are not specific to any storage provider.

use crate::bundle_config::{config_keys, config_scopes, ConfigKey, ConfigScope};
use crate::BundleConfig;

// The system scope for bundlebase-level settings.
config_scopes!(system_scopes, {
    pub const SYSTEM_SCOPE: ConfigScope =
        BundleConfig::register_scope("system");
});

config_keys!(system_keys, {
    //todo: use these
    pub const MAX_MEMORY_CFG: ConfigKey = SYSTEM_SCOPE.define("max_memory");
    pub const CATALOG_NAME_CFG: ConfigKey = SYSTEM_SCOPE.define("catalog_name");
});

