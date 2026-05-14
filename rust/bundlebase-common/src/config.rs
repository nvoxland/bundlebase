//! Configuration types and traits for Bundlebase.
//!
//! This module provides the foundational config types (`ConfigScope`, `ConfigKey`,
//! `ConfigSource`, `Scope`) and the `ConfigProvider` trait that allows crates
//! to read configuration without depending on the full `BundleConfig` implementation.

mod scope;

pub use scope::Scope;

use crate::BundlebaseError;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Identifies which config layer a value came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConfigSource {
    /// Stored in the bundle manifest via SaveConfigOp
    Stored,
    /// From environment variables (BB_*)
    Env,
    /// Passed explicitly to create()/open()
    Passed,
    /// Set at runtime via SET CONFIG (session-only)
    Runtime,
    /// Static default defined on a ConfigKey via `with_default()`
    Default,
}

impl ConfigSource {
    /// String representation for Python/CLI display.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConfigSource::Stored => "stored",
            ConfigSource::Env => "env",
            ConfigSource::Passed => "passed",
            ConfigSource::Runtime => "runtime",
            ConfigSource::Default => "default",
        }
    }

    /// Higher priority wins when the same key+scope appears in multiple layers.
    pub fn priority(&self) -> u8 {
        match self {
            ConfigSource::Default => 0,
            ConfigSource::Stored => 1,
            ConfigSource::Env => 2,
            ConfigSource::Passed => 3,
            ConfigSource::Runtime => 4,
        }
    }
}

/// Identifies which provider/protocol a configuration key belongs to.
///
/// Created via `ConfigScope::new("s3")`. At runtime, `matches()`
/// checks that a `Scope` (e.g. `s3/bucket`) falls under this config scope.
///
/// Each scope carries a function pointer for converting URLs to scope names.
/// The default (`default_url_to_name`) matches URLs whose scheme equals
/// the scope name (e.g., `s3://…`). Providers that use custom URL formats
/// (like Kaggle) override this via `with_url_to_name()`.
#[derive(Debug, Clone, Copy, Eq)]
pub struct ConfigScope {
    /// Provider name (e.g., "s3", "gs", "azure", "sftp", "kaggle")
    pub name: &'static str,
    /// Function to convert a URL to a scope name for this provider.
    /// Takes (&ConfigScope, &str) → Option<String>.
    /// Returns Some(name) if the URL belongs to this scope, None otherwise.
    url_to_name_fn: fn(&ConfigScope, &str) -> Option<String>,
}

impl PartialEq for ConfigScope {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

/// Default URL-to-name conversion: matches URLs whose scheme equals the scope name.
///
/// Only handles URL-format input (e.g., `s3://bucket/path` → `"s3/bucket/path"`).
/// Name-based input (e.g., `"s3/bucket"`) is handled by `Scope::try_from` directly.
pub fn default_url_to_name(scope: &ConfigScope, url: &str) -> Option<String> {
    let url_prefix = format!("{}://", scope.name);
    if !url.starts_with(&url_prefix) {
        return None;
    }
    let rest = url[url_prefix.len()..].trim_end_matches('/');
    if rest.is_empty() {
        Some(scope.name.to_string())
    } else {
        Some(format!("{}/{}", scope.name, rest))
    }
}

impl ConfigScope {
    /// Create a new configuration scope.
    ///
    /// This is a const fn that creates the scope with the default URL-to-name conversion.
    /// ```rust,ignore
    /// pub const S3_SCOPE: ConfigScope = ConfigScope::new("s3");
    /// ```
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            url_to_name_fn: default_url_to_name,
        }
    }

    /// Convert a URL to a scope name using this scope's rules.
    /// Returns Some(name) if the URL belongs to this scope, None otherwise.
    pub fn url_to_name(&self, url: &str) -> Option<String> {
        (self.url_to_name_fn)(self, url)
    }

    /// Builder: override the URL-to-name conversion function.
    pub const fn with_url_to_name(mut self, f: fn(&ConfigScope, &str) -> Option<String>) -> Self {
        self.url_to_name_fn = f;
        self
    }

    /// Define a configuration key within this scope.
    ///
    /// Defaults: persistence is `Either` (any source allowed) and `secure`
    /// is false (display value unmasked). Use the `.stored_only()`,
    /// `.runtime_only()`, and `.secure()` builders on the returned key to
    /// adjust.
    ///
    /// ```rust,ignore
    /// pub const S3_REGION_CFG: ConfigKey = S3_SCOPE.define("region");
    /// pub const S3_SECRET_CFG: ConfigKey =
    ///     S3_SCOPE.define("secret_access_key").runtime_only().secure();
    /// ```
    pub const fn define(self, key: &'static str) -> ConfigKey {
        ConfigKey {
            key,
            secure: false,
            persistence: ConfigPersistence::Either,
            scope: self,
            default_value: None,
            default_fn: None,
        }
    }

    /// Check if a runtime `Scope` is compatible with this `ConfigScope`.
    ///
    /// e.g., `ConfigScope("s3")` matches `Scope("s3")`, `Scope("s3/bucket")`, etc.
    /// but NOT `Scope("")` (global) or `Scope("gs/bucket")`.
    pub fn matches(&self, scope: &Scope) -> bool {
        let s = scope.as_str();
        let n = self.name;
        // Exact match: "s3" == "s3"
        if s == n {
            return true;
        }
        // Prefix match: "s3/bucket" starts with "s3/"
        if s.starts_with(n) && s.as_bytes().get(n.len()) == Some(&b'/') {
            return true;
        }
        false
    }
}

impl std::fmt::Display for ConfigScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Where a config key's value is allowed to come from.
///
/// `secure` is independent of this — it only affects display masking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPersistence {
    /// Any source allowed: Stored (manifest), Passed, Env, Runtime. Default.
    Either,
    /// Must come from a `SaveConfigOp` in the bundle manifest. Passed,
    /// Env, and Runtime sources are rejected. Use for settings that
    /// affect bundle content/format and must travel with the bundle (e.g.
    /// `system.git_versioning`).
    StoredOnly,
    /// Must come from Passed/Env/Runtime. `SaveConfigOp` is rejected. Use
    /// for trust/safety toggles a bundle must not be able to enable for
    /// itself (e.g. `system.allow_external_code`), and for secrets that
    /// should never be written to a manifest.
    RuntimeOnly,
}

/// Defines a known configuration key.
///
/// Each service/provider defines its own slice of `ConfigKey` entries.
/// Duplicate keys across modules are fine (e.g., `access_key` in S3 and Azure).
#[derive(Debug, Clone, Copy)]
pub struct ConfigKey {
    /// Configuration key name (e.g., "region", "secret_access_key")
    pub key: &'static str,
    /// Whether this key's value should be masked in `SHOW CONFIG` output.
    /// Independent of `persistence`.
    pub secure: bool,
    /// Which sources are allowed to provide a value for this key.
    pub persistence: ConfigPersistence,
    /// Which provider scope this key belongs to
    pub scope: ConfigScope,
    /// Static default value, set via `with_default()`.
    pub default_value: Option<&'static str>,
    /// Dynamic default: `(description, resolver)`, set via `with_default_fn()`.
    pub default_fn: Option<(&'static str, fn() -> Option<String>)>,
}

impl ConfigKey {
    /// Set a static default value for this config key.
    pub const fn with_default(mut self, value: &'static str) -> Self {
        self.default_value = Some(value);
        self
    }

    /// Set a dynamic default for this config key.
    pub const fn with_default_fn(
        mut self,
        description: &'static str,
        f: fn() -> Option<String>,
    ) -> Self {
        self.default_fn = Some((description, f));
        self
    }

    /// Resolve the default value for this key.
    ///
    /// Checks `default_fn` first (dynamic), then `default_value` (static).
    pub fn resolve_default(&self) -> Option<String> {
        if let Some((_desc, f)) = self.default_fn {
            return f();
        }
        self.default_value.map(|v| v.to_string())
    }

    /// Description string for display in `SHOW CONFIG`.
    pub fn default_description(&self) -> Option<&'static str> {
        if let Some((desc, _)) = self.default_fn {
            return Some(desc);
        }
        self.default_value
    }

    /// Restrict this key to the `Stored` source (manifest only).
    pub const fn stored_only(mut self) -> Self {
        self.persistence = ConfigPersistence::StoredOnly;
        self
    }

    /// Restrict this key to non-`Stored` sources (Passed/Env/Runtime).
    pub const fn runtime_only(mut self) -> Self {
        self.persistence = ConfigPersistence::RuntimeOnly;
        self
    }

    /// Mark this key's value as a secret, so `SHOW CONFIG` displays
    /// `*****` instead of the actual value. Source policy is independent —
    /// most secure keys also want `.runtime_only()`.
    pub const fn secure(mut self) -> Self {
        self.secure = true;
        self
    }

    /// Validate that `source` is an allowed source for this key's
    /// persistence policy. `ConfigSource::Default` is always allowed.
    /// `StoredOnly` rejects non-Stored sources; `RuntimeOnly` rejects
    /// Stored; `Either` is unrestricted.
    pub fn validate_source(&self, source: &ConfigSource) -> Result<(), BundlebaseError> {
        if source == &ConfigSource::Default {
            return Ok(());
        }
        match self.persistence {
            ConfigPersistence::Either => Ok(()),
            ConfigPersistence::StoredOnly => {
                if source == &ConfigSource::Stored {
                    Ok(())
                } else {
                    Err(format!(
                        "Config key '{}.{}' must be set in the bundle manifest \
                         (use SAVE CONFIG). It cannot be set via {}.",
                        self.scope.name,
                        self.key,
                        source.as_str()
                    )
                    .into())
                }
            }
            ConfigPersistence::RuntimeOnly => {
                if source == &ConfigSource::Stored {
                    Err(format!(
                        "Config key '{}.{}' cannot be saved in the bundle manifest. \
                         Set it at runtime via passed config, environment variable, \
                         or SET CONFIG.",
                        self.scope.name, self.key
                    )
                    .into())
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// Function signature for a config change hook. Receives a type-erased
/// reference to the builder driving the change (callers pass
/// `&BundleBuilder as &dyn Any` from the `bundlebase` crate) plus the old
/// and new config values (either may be `None` if the key was previously
/// unset or is being cleared) and returns a boxed future. The hook runs
/// after the `SaveConfigOp` has been applied, so reads of the new value
/// reflect the transition.
///
/// The caller guarantees `old != new` — hooks don't need to recheck.
/// The hook downcasts the `&dyn Any` to whatever concrete builder type
/// it knows about (typically `&BundleBuilder` from the `bundlebase`
/// crate) and does whatever it wants with old/new.
pub type ConfigChangeHookFn = for<'a> fn(
    &'a (dyn std::any::Any + Send + Sync),
    Option<&'a str>,
    Option<&'a str>,
) -> BoxFuture<'a, Result<(), BundlebaseError>>;

/// Subscribe to value transitions on a config key.
///
/// Hooks fire after the `SaveConfigOp` has been applied to the
/// in-progress builder change, only when `old != new`. Multiple hooks
/// can be registered on the same key; they fire in registration order.
/// Use for keys whose value affects derived state that has to be
/// reconciled on flip (e.g. `system.git_versioning` triggers
/// `refresh_block_versions` from the `bundlebase` crate).
///
/// Built-in hooks are registered during config registry initialization;
/// downstream crates can call this at any time to add their own.
pub mod change_hook {
    use super::{ConfigChangeHookFn, ConfigKey};
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::OnceLock;

    type HookKey = (&'static str, &'static str);
    type HookMap = RwLock<HashMap<HookKey, Vec<ConfigChangeHookFn>>>;

    fn registry() -> &'static HookMap {
        static R: OnceLock<HookMap> = OnceLock::new();
        R.get_or_init(|| RwLock::new(HashMap::new()))
    }

    /// Register `hook` to fire when the given key's value transitions.
    /// Idempotent at the key level — calling multiple times appends
    /// additional listeners.
    pub fn add(key: &ConfigKey, hook: ConfigChangeHookFn) {
        registry()
            .write()
            .entry((key.scope.name, key.key))
            .or_default()
            .push(hook);
    }

    /// All hooks registered for the given scope name + key name, in
    /// registration order.
    pub fn get(scope_name: &str, key_name: &str) -> Vec<ConfigChangeHookFn> {
        registry()
            .read()
            .iter()
            .find(|(&(s, k), _)| s == scope_name && k == key_name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

/// Declares `pub const` config keys and generates a function returning `&'static [ConfigKey]`.
///
/// # Example
/// ```rust,ignore
/// config_keys!(s3_keys, {
///     pub const S3_REGION_CFG: ConfigKey = S3_SCOPE.define("region");
///     pub const S3_SECRET_CFG: ConfigKey =
///         S3_SCOPE.define("secret").runtime_only().secure();
/// });
/// ```
#[macro_export]
macro_rules! config_keys {
    ($fn_name:ident, {
        $( pub const $name:ident : ConfigKey = $init:expr ; )*
    }) => {
        $( pub const $name: ConfigKey = $init; )*

        pub fn $fn_name() -> &'static [ConfigKey] {
            &[ $( $name ),* ]
        }
    };
}

/// Declares `pub const` config scopes and generates a function returning `&'static [ConfigScope]`.
///
/// # Example
/// ```rust,ignore
/// config_scopes!(object_store_scopes, {
///     pub const S3_SCOPE: ConfigScope = ConfigScope::new("s3");
///     pub const GCS_SCOPE: ConfigScope = ConfigScope::new("gs");
/// });
/// ```
#[macro_export]
macro_rules! config_scopes {
    ($fn_name:ident, {
        $( pub const $name:ident : ConfigScope = $init:expr ; )*
    }) => {
        $( pub const $name: ConfigScope = $init; )*

        pub fn $fn_name() -> &'static [ConfigScope] {
            &[ $( $name ),* ]
        }
    };
}

/// Trait for reading configuration values.
///
/// This trait abstracts config access so that crates like `bundlebase-io` can
/// read configuration without depending on the full `BundleConfig` implementation.
pub trait ConfigProvider: Send + Sync {
    /// Get the winning value for a key, looking up under the given
    /// (possibly sub-namespaced) scope. Use for providers like S3
    /// where a key can be set at `s3` and overridden at `s3/bucket-foo`.
    fn get_in_scope(
        &self,
        scope: &Scope,
        key: &ConfigKey,
    ) -> Result<Option<String>, BundlebaseError>;

    /// Get the winning value for a key in the key's own provider scope.
    /// Convenience for non-namespaced configs (like `system.*`) where
    /// the scope is fully determined by the key constant.
    fn get(&self, key: &ConfigKey) -> Result<Option<String>, BundlebaseError> {
        let scope = Scope::try_from(key.scope.name)?;
        self.get_in_scope(&scope, key)
    }

    /// Like `get_in_scope`, but errors if the key has no value.
    fn get_required_in_scope(
        &self,
        scope: &Scope,
        key: &ConfigKey,
        context: &str,
    ) -> Result<String, BundlebaseError> {
        self.get_in_scope(scope, key)?.ok_or_else(|| {
            BundlebaseError::from(format!(
                "{}: No configuration set for /{}:{}",
                context, key.scope.name, key.key
            ))
        })
    }

    /// Like `get`, but errors if the key has no value.
    fn get_required(
        &self,
        key: &ConfigKey,
        context: &str,
    ) -> Result<String, BundlebaseError> {
        let scope = Scope::try_from(key.scope.name)?;
        self.get_required_in_scope(&scope, key, context)
    }
}

/// Blanket impl for Arc<T> where T: ConfigProvider
impl<T: ConfigProvider + ?Sized> ConfigProvider for Arc<T> {
    fn get_in_scope(
        &self,
        scope: &Scope,
        key: &ConfigKey,
    ) -> Result<Option<String>, BundlebaseError> {
        (**self).get_in_scope(scope, key)
    }
}
