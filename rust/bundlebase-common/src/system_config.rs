//! System scope configuration constants.
//!
//! These are bundlebase-level settings not specific to any storage provider.

use crate::config::{ConfigKey, ConfigScope, Scope};
use crate::{config_keys, config_scopes, BundlebaseError, ConfigProvider};

config_scopes!(system_scopes, {
    pub const SYSTEM_SCOPE: ConfigScope = ConfigScope::new("system");
});

config_keys!(system_keys, {
    pub const MAX_MEMORY_CFG: ConfigKey = SYSTEM_SCOPE.define("max_memory");
    pub const CATALOG_NAME_CFG: ConfigKey = SYSTEM_SCOPE.define("catalog_name");
    pub const ALLOW_EXTERNAL_CODE_CFG: ConfigKey = SYSTEM_SCOPE
        .define("allow_external_code")
        .runtime_only()
        .with_default("false");
    pub const ALLOW_INTERNAL_SOURCES_CFG: ConfigKey = SYSTEM_SCOPE
        .define("allow_internal_sources")
        .runtime_only()
        .with_default("false");
    pub const GIT_VERSIONING_CFG: ConfigKey = SYSTEM_SCOPE
        .define("git_versioning")
        .stored_only()
        .with_default("false");
});

/// Returns `true` if the `system.allow_external_code` config is set to `"true"`.
pub fn is_external_code_allowed(config: &dyn ConfigProvider) -> Result<bool, BundlebaseError> {
    let value = config.get(&ALLOW_EXTERNAL_CODE_CFG)?;
    Ok(value.as_deref() == Some("true"))
}

/// Returns `true` if `system.allow_internal_sources` is set to `"true"`.
///
/// When false (the default), source URLs that target internal/metadata
/// endpoints (loopback, link-local, cloud metadata) are refused so opening an
/// untrusted bundle can't SSRF the consumer's network. Operators who genuinely
/// pull from a local/internal service can opt in.
pub fn is_internal_sources_allowed(config: &dyn ConfigProvider) -> Result<bool, BundlebaseError> {
    let value = config.get(&ALLOW_INTERNAL_SOURCES_CFG)?;
    Ok(value.as_deref() == Some("true"))
}
