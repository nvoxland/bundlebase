//! System scope configuration.
//!
//! The system scope provides bundlebase-level settings like memory limits
//! and catalog naming that are not specific to any storage provider.

use crate::bundle_config::{config_keys, config_scopes, ConfigKey, ConfigScope, Scope};
use crate::{BundleConfig, BundlebaseError};

// The system scope for bundlebase-level settings.
config_scopes!(system_scopes, {
    pub const SYSTEM_SCOPE: ConfigScope =
        BundleConfig::register_scope("system");
});

config_keys!(system_keys, {
    //todo: use these
    pub const MAX_MEMORY_CFG: ConfigKey = SYSTEM_SCOPE.define("max_memory");
    pub const CATALOG_NAME_CFG: ConfigKey = SYSTEM_SCOPE.define("catalog_name");
    pub const ALLOW_EXTERNAL_CODE_CFG: ConfigKey = SYSTEM_SCOPE
        .define("allow_external_code")
        .with_default("false");
});

/// Returns `true` if the `system.allow_external_code` config is set to `"true"`.
pub fn is_external_code_allowed(config: &BundleConfig) -> Result<bool, BundlebaseError> {
    let scope = Scope::try_from(SYSTEM_SCOPE.name)?;
    let value = config.get(&scope, &ALLOW_EXTERNAL_CODE_CFG)?;
    Ok(value.as_deref() == Some("true"))
}

