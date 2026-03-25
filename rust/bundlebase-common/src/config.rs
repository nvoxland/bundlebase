//! Configuration types and traits for Bundlebase.
//!
//! This module provides the foundational config types (`ConfigScope`, `ConfigKey`,
//! `ConfigSource`, `Scope`) and the `ConfigProvider` trait that allows crates
//! to read configuration without depending on the full `BundleConfig` implementation.

mod scope;

pub use scope::Scope;

use crate::BundlebaseError;
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
    pub const fn with_url_to_name(
        mut self,
        f: fn(&ConfigScope, &str) -> Option<String>,
    ) -> Self {
        self.url_to_name_fn = f;
        self
    }

    /// Define a non-secure configuration key within this scope.
    ///
    /// ```rust,ignore
    /// pub const S3_REGION_CFG: ConfigKey = S3_SCOPE.define("region");
    /// ```
    pub const fn define(self, key: &'static str) -> ConfigKey {
        ConfigKey {
            key,
            secure: false,
            scope: self,
            default_value: None,
            default_fn: None,
        }
    }

    /// Define a secure (secret) configuration key within this scope.
    ///
    /// Values for secure keys are masked in display output.
    /// ```rust,ignore
    /// pub const S3_SECRET: ConfigKey = S3_SCOPE.define_secure("secret_access_key");
    /// ```
    pub const fn define_secure(self, key: &'static str) -> ConfigKey {
        ConfigKey {
            key,
            secure: true,
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

/// Defines a known configuration key and whether it is secure.
///
/// Each service/provider defines its own slice of `ConfigKey` entries.
/// Duplicate keys across modules are fine (e.g., `access_key` in S3 and Azure).
#[derive(Debug, Clone, Copy)]
pub struct ConfigKey {
    /// Configuration key name (e.g., "region", "secret_access_key")
    pub key: &'static str,
    /// Whether this key holds a secret (password, token, etc.)
    pub secure: bool,
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
}

/// Declares `pub const` config keys and generates a function returning `&'static [ConfigKey]`.
///
/// # Example
/// ```rust,ignore
/// config_keys!(s3_keys, {
///     pub const S3_REGION_CFG: ConfigKey = S3_SCOPE.define("region");
///     pub const S3_SECRET_CFG: ConfigKey = S3_SCOPE.define_secure("secret");
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
    /// Get the winning value for a key, scoped to a parsed Scope.
    fn get(&self, scope: &Scope, key: &ConfigKey) -> Result<Option<String>, BundlebaseError>;

    /// Like `get`, but returns an error if the key is not set.
    fn get_required(
        &self,
        scope: &Scope,
        key: &ConfigKey,
        context: &str,
    ) -> Result<String, BundlebaseError> {
        self.get(scope, key)?.ok_or_else(|| {
            BundlebaseError::from(format!(
                "{}: No configuration set for /{}:{}",
                context, key.scope.name, key.key
            ))
        })
    }
}

/// Blanket impl for Arc<T> where T: ConfigProvider
impl<T: ConfigProvider + ?Sized> ConfigProvider for Arc<T> {
    fn get(&self, scope: &Scope, key: &ConfigKey) -> Result<Option<String>, BundlebaseError> {
        (**self).get(scope, key)
    }
}
