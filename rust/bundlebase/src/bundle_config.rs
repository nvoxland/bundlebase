mod passed;
mod scope;
pub use passed::PassedBundleConfig;
pub use scope::Scope;

use arrow::array::{BooleanArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use std::sync::Arc;

use crate::bundle::command::response::{single_batch_stream, CommandResponse, OutputShape};
use crate::impl_dyn_command_response;
use crate::BundlebaseError;
use datafusion::execution::SendableRecordBatchStream;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

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
    fn priority(&self) -> u8 {
        match self {
            ConfigSource::Default => 0,
            ConfigSource::Stored => 1,
            ConfigSource::Env => 2,
            ConfigSource::Passed => 3,
            ConfigSource::Runtime => 4,
        }
    }
}

/// A single config entry with source tracking metadata.
#[derive(Debug, Clone)]
pub struct ConfigValueDetails {
    /// Configuration key (e.g., "region", "endpoint")
    pub key: String,
    /// Configuration value
    pub value: String,
    /// from_pathd scope, or global (`/`) for defaults
    pub scope: Scope,
    /// Which layer this value came from
    pub source: ConfigSource,
    /// True if this entry is the winning value for its key+scope
    pub active: bool,
    /// True if this key holds a secret (password, token, etc.)
    pub secure: bool,
}

/// Identifies which provider/protocol a configuration key belongs to.
///
/// Created via `BundleConfig::register_scope("s3")`. At runtime, `matches()`
/// checks that a `Scope` (e.g. `/s3/bucket`) falls under this config scope.
///
/// Each scope carries a function pointer for converting paths to scopes.
/// The default (`default_scope_from_path`) matches paths whose scheme equals
/// the scope name (e.g., `s3://…`). Providers that use custom path formats
/// (like Kaggle) override this via `with_from_path()`.
#[derive(Debug, Clone, Copy, Eq)]
pub struct ConfigScope {
    /// Provider name (e.g., "s3", "gs", "azure", "sftp", "kaggle")
    pub name: &'static str,
    /// Function to convert a path to a Scope for this provider.
    /// Takes (&ConfigScope, &str) → Option<Scope>.
    /// Returns Some if the path belongs to this scope, None otherwise.
    from_path_fn: fn(&ConfigScope, &str) -> Option<Scope>,
}

impl PartialEq for ConfigScope {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

/// Default path-to-scope conversion: matches paths whose scheme equals the scope name
/// or that are already in from_pathd form.
///
/// Matches both URL-format (`s3://bucket/path`) and from_pathd (`/s3/bucket/path`) paths.
pub fn default_scope_from_path(scope: &ConfigScope, path: &str) -> Option<Scope> {
    // Match URL-format paths: "s3://bucket/path"
    let url_prefix = format!("{}://", scope.name);
    if path.starts_with(&url_prefix) {
        return Some(Scope::from_path(path).expect("Unknown base scope"));
    }
    // Match already-from_pathd paths: "/s3" or "/s3/bucket/path"
    let norm_prefix = format!("/{}", scope.name);
    if path == norm_prefix || path.starts_with(&format!("{}/", norm_prefix)) {
        return Some(Scope::new(path));
    }
    None
}

impl ConfigScope {
    /// Convert a path to a Scope using this scope's rules.
    /// Returns Some if the path belongs to this scope.
    pub fn from_path(&self, path: &str) -> Option<Scope> {
        (self.from_path_fn)(self, path)
    }

    /// Builder: override the path-to-scope conversion function.
    pub const fn with_from_path(
        mut self,
        f: fn(&ConfigScope, &str) -> Option<Scope>,
    ) -> Self {
        self.from_path_fn = f;
        self
    }

    /// Define a non-secure configuration key within this scope.
    ///
    /// ```rust,ignore
    /// pub const S3_REGION_CFG: ConfigKey = S3_SCOPE.define("region");
    /// ```
    pub const fn define(self, key: &'static str) -> ConfigKey {
        ConfigKey { key, secure: false, scope: self, default_value: None, default_fn: None }
    }

    /// Define a secure (secret) configuration key within this scope.
    ///
    /// Values for secure keys are masked in display output.
    /// ```rust,ignore
    /// pub const S3_SECRET: ConfigKey = S3_SCOPE.define_secure("secret_access_key");
    /// ```
    pub const fn define_secure(self, key: &'static str) -> ConfigKey {
        ConfigKey { key, secure: true, scope: self, default_value: None, default_fn: None }
    }

    /// Check if a runtime `Scope` is compatible with this `ConfigScope`.
    ///
    /// e.g., `ConfigScope("s3")` matches `Scope("/s3")`, `Scope("/s3/bucket")`, etc.
    /// but NOT `Scope("/")` (global) or `Scope("/gs/bucket")`.
    pub fn matches(&self, scope: &Scope) -> bool {
        let s = scope.as_str().as_bytes();
        let n = self.name.as_bytes();
        s.len() >= 1 + n.len()
            && s[0] == b'/'
            && s[1..1 + n.len()] == *n
            && (s.len() == 1 + n.len() || s[1 + n.len()] == b'/')
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
    /// The value is both the display value in `SHOW CONFIG` and the actual default.
    pub default_value: Option<&'static str>,
    /// Dynamic default: `(description, resolver)`, set via `with_default_fn()`.
    ///
    /// The resolver function takes no arguments and returns the actual default
    /// (or `None` if unavailable). The description is shown in `SHOW CONFIG`
    /// output (e.g., `"~/.kaggle/kaggle.json"`).
    pub default_fn: Option<(&'static str, fn() -> Option<String>)>,
}

impl ConfigKey {
    /// Set a static default value for this config key.
    ///
    /// When `BundleConfig::get()` finds no value from any source, it returns
    /// this default if the key's scope is compatible with the lookup scope.
    ///
    /// ```rust,ignore
    /// pub const MY_KEY: ConfigKey = MY_SCOPE.define("base_url")
    ///     .with_default("https://example.com");
    /// ```
    pub const fn with_default(mut self, value: &'static str) -> Self {
        self.default_value = Some(value);
        self
    }

    /// Set a dynamic default for this config key.
    ///
    /// The resolver function is called with the description string and returns
    /// the actual default value (or `None` if unavailable). The description is
    /// displayed in `SHOW CONFIG` output (e.g., `"~/.kaggle/kaggle.json"`).
    ///
    /// ```rust,ignore
    /// fn read_username(_desc: &'static str) -> Option<String> { ... }
    /// pub const MY_KEY: ConfigKey = MY_SCOPE.define("username")
    ///     .with_default_fn("~/.kaggle/kaggle.json", read_username);
    /// ```
    pub const fn with_default_fn(mut self, description: &'static str, f: fn() -> Option<String>) -> Self {
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
    ///
    /// For static defaults this is the value itself; for dynamic defaults
    /// it is the description passed to `with_default_fn`.
    pub fn default_description(&self) -> Option<&'static str> {
        if let Some((desc, _)) = self.default_fn {
            return Some(desc);
        }
        self.default_value
    }

    /// Check whether a key is secure.
    ///
    /// Returns true if ANY spec marks the key as secure.
    pub fn is_key_secure(key: &str, specs: &[ConfigKey]) -> bool {
        specs.iter().any(|spec| spec.key == key && spec.secure)
    }

    /// Check whether a key is recognized by any spec.
    pub fn is_key_valid(key: &str, specs: &[ConfigKey]) -> bool {
        specs.iter().any(|spec| spec.key == key)
    }

    /// Validate that a config key is recognized.
    ///
    /// Returns an error if the key is not found in any spec.
    pub fn validate_key(key: &str, specs: &[ConfigKey]) -> Result<(), BundlebaseError> {
        if Self::is_key_valid(key, specs) {
            Ok(())
        } else {
            let all_keys: Vec<&str> = specs.iter().map(|s| s.key).collect();
            Err(format!(
                "Invalid config key '{}'. Valid keys: {:?}",
                key, all_keys
            )
            .into())
        }
    }

    /// Validate that a config key is recognized for a specific scope.
    ///
    /// Returns an error if the key is not found in any spec matching the scope.
    pub fn validate_key_scoped(key: &str, scope: &Scope, specs: &[ConfigKey]) -> Result<(), BundlebaseError> {
        if specs.iter().any(|s| s.key == key && s.scope.matches(scope)) {
            Ok(())
        } else {
            let valid_keys: Vec<&str> = specs
                .iter()
                .filter(|s| s.scope.matches(scope))
                .map(|s| s.key)
                .collect();
            Err(format!(
                "Unknown config key '{}' for scope '{}'. Valid keys for this scope: {:?}",
                key, scope, valid_keys
            )
            .into())
        }
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
/// // Generates: pub const S3_REGION_CFG, S3_SECRET_CFG, and fn s3_keys() -> &'static [ConfigKey]
/// ```
macro_rules! config_keys {
    ($fn_name:ident, {
        $( pub const $name:ident : ConfigKey = $init:expr ; )*
    }) => {
        $( pub const $name: ConfigKey = $init; )*

        pub(crate) fn $fn_name() -> &'static [ConfigKey] {
            &[ $( $name ),* ]
        }
    };
}
pub(crate) use config_keys;

/// Declares `pub const` config scopes and generates a function returning `&'static [ConfigScope]`.
///
/// # Example
/// ```rust,ignore
/// config_scopes!(object_store_scopes, {
///     pub const S3_SCOPE: ConfigScope = BundleConfig::register_scope("s3");
///     pub const GCS_SCOPE: ConfigScope = BundleConfig::register_scope("gs");
/// });
/// // Generates: pub const S3_SCOPE, GCS_SCOPE, and fn object_store_scopes() -> &'static [ConfigScope]
/// ```
macro_rules! config_scopes {
    ($fn_name:ident, {
        $( pub const $name:ident : ConfigScope = $init:expr ; )*
    }) => {
        $( pub const $name: ConfigScope = $init; )*

        pub(crate) fn $fn_name() -> &'static [ConfigScope] {
            &[ $( $name ),* ]
        }
    };
}
pub(crate) use config_scopes;

/// A single config entry stored internally.
#[derive(Debug, Clone)]
struct ConfigValue {
    key: String,
    value: String,
    /// "" = global default, non-empty = scope-specific
    scope: String,
}

/// A winning config entry for a specific key, used in the active cache.
#[derive(Debug, Clone)]
struct ActiveEntry {
    scope: Scope,
    value: String,
}

/// Internal mutable state behind the RwLock.
#[derive(Debug)]
struct ConfigInner {
    /// All config entries, grouped by source.
    /// Within each source, entries are stored in insertion order (last wins for same key+scope).
    entries: HashMap<ConfigSource, Vec<ConfigValue>>,
    /// Whether env vars have been loaded into entries[Env].
    env_loaded: bool,
    /// Cached active entries: key -> list of winning (scope, value) entries,
    /// sorted by scope length descending (longest prefix first, global last).
    /// None = cache stale, Some = populated.
    active_cache: Option<HashMap<String, Vec<ActiveEntry>>>,
    /// Cached winners from compute_winners(), stored alongside active_cache
    /// so all_values() doesn't need to recompute them.
    /// Populated and invalidated together with active_cache.
    winners_cache: Option<HashMap<(String, Scope), (u8, String)>>,
}

impl ConfigInner {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            env_loaded: false,
            active_cache: None,
            winners_cache: None,
        }
    }
}

/// Configuration for container storage and cloud providers.
///
/// `BundleConfig` is the single, self-contained, internally thread-safe holder
/// for all config data. It uses interior mutability via `parking_lot::RwLock`
/// so all methods take `&self`.
///
/// # Config Sources (priority order, highest first)
/// 1. **Runtime** — `SET CONFIG` (session-only)
/// 2. **Passed** — config passed to `create()`/`open()`
/// 3. **Env** — environment variables (`BB_*`), lazily loaded
/// 4. **Stored** — saved in bundle manifest via `SaveConfigOp`
/// ```
pub struct BundleConfig {
    inner: RwLock<ConfigInner>,
}

impl std::fmt::Debug for BundleConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read();
        f.debug_struct("BundleConfig")
            .field("entries", &inner.entries)
            .finish()
    }
}

impl BundleConfig {
    /// Register a configuration scope for a provider.
    ///
    /// ```rust,ignore
    /// pub const S3_SCOPE: ConfigScope = BundleConfig::register_scope("s3");
    /// ```
    pub const fn register_scope(name: &'static str) -> ConfigScope {
        ConfigScope { name, from_path_fn: default_scope_from_path }
    }

    /// Returns all known configuration scopes.
    pub fn all_scopes() -> Vec<ConfigScope> {
        use crate::io::plugin::ftp;
        use crate::io::plugin::object_store;
        use crate::io::plugin::sftp;
        use crate::source::kaggle;

        let mut scopes = Vec::new();
        scopes.extend_from_slice(object_store::object_store_scopes());
        scopes.extend_from_slice(ftp::ftp_scopes());
        scopes.extend_from_slice(sftp::sftp_scopes());
        scopes.extend_from_slice(kaggle::scopes());
        scopes
    }

    /// Returns all known configuration key specs for validation.
    ///
    /// Each provider defines its keys alongside its implementation.
    /// This collects them all for use by `BundleConfig::from_map()`.
    pub fn all_keys() -> Vec<ConfigKey> {
        use crate::io::plugin::ftp;
        use crate::io::plugin::object_store;
        use crate::io::plugin::sftp;
        use crate::source::kaggle;

        let mut specs = Vec::new();
        specs.extend_from_slice(object_store::s3_keys());
        specs.extend_from_slice(object_store::gcs_keys());
        specs.extend_from_slice(object_store::azure_keys());
        specs.extend_from_slice(ftp::ftp_keys());
        specs.extend_from_slice(sftp::sftp_keys());
        specs.extend_from_slice(kaggle::configs());
        specs
    }

    /// Create a new empty configuration. No env loading.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(ConfigInner::new()),
        }
    }

    /// Set a config value.
    ///
    /// # Arguments
    /// * `key` - Configuration key (e.g., "region", "access_key_id").
    /// * `value` - Configuration value
    /// * `scope` - from_pathd scope, or global for default.
    ///             Use `Scope::from_path()` to convert raw paths at the call site.
    /// * `source` - Which config layer this entry belongs to
    pub fn set(&self, key: &str, value: &str, scope: &Scope, source: ConfigSource) {
        let mut inner = self.inner.write();
        let scope_str = scope.as_str().to_string();

        let entries = inner.entries.entry(source).or_default();

        // Remove any existing entry with the same key+scope (last write wins)
        entries.retain(|e| !(e.key == key && e.scope == scope_str));

        entries.push(ConfigValue {
            key: key.to_string(),
            value: value.to_string(),
            scope: scope_str,
        });
        inner.active_cache = None;
        inner.winners_cache = None;
    }

    /// Replace all inner state except Runtime entries (for `reload_from`).
    pub fn reload_non_runtime(&self, other: &BundleConfig) {
        let other_inner = other.inner.read();
        let mut self_inner = self.inner.write();

        // Preserve runtime entries
        let runtime = self_inner.entries.remove(&ConfigSource::Runtime);

        // Replace everything from other
        self_inner.entries = other_inner.entries.clone();
        self_inner.env_loaded = false;
        self_inner.entries.remove(&ConfigSource::Env);

        // Restore runtime entries
        if let Some(runtime_entries) = runtime {
            self_inner.entries.insert(ConfigSource::Runtime, runtime_entries);
        }
        self_inner.active_cache = None;
        self_inner.winners_cache = None;
    }

    /// Get the winning value for a key, scoped to a from_pathd Scope.
    ///
    /// Ensures env cache is populated, then finds the longest matching prefix
    /// across all sources. Among entries sharing the longest prefix, the
    /// highest-priority source wins. Only entries whose scope is compatible
    /// with the key's required `ConfigScope` are considered.
    pub fn get(&self, scope: &Scope, key: &ConfigKey) -> Option<String> {
        self.ensure_env_cache();

        // Fast path: check active cache with read lock
        {
            let inner = self.inner.read();
            if let Some(cache) = &inner.active_cache {
                if let Some(value) = Self::lookup_active(cache, key, scope) {
                    return Some(value);
                }
                // Fall back to default if scope is compatible
                if key.scope.matches(scope) {
                    if let Some(value) = key.resolve_default() {
                        return Some(value);
                    }
                }
                return None;
            }
        }

        // Slow path: populate cache with write lock
        let mut inner = self.inner.write();

        // Double-check after upgrading to write lock
        if inner.active_cache.is_none() {
            Self::populate_active_cache(&mut inner);
        }

        match &inner.active_cache {
            Some(cache) => {
                if let Some(value) = Self::lookup_active(cache, key, scope) {
                    return Some(value);
                }
                // Fall back to default if scope is compatible
                if key.scope.matches(scope) {
                    if let Some(value) = key.resolve_default() {
                        return Some(value);
                    }
                }
                None
            }
            None => None, // should not happen after populate_active_cache
        }
    }

    /// Like [`get`], but returns an error if the key is not set.
    ///
    /// `context` is prepended to the error message (e.g. `"Cannot configure Kaggle client: No configuration set for /kaggle:username"`).
    pub fn get_required(&self, scope: &Scope, key: &ConfigKey, context: &str) -> Result<String, BundlebaseError> {
        self.get(scope, key).ok_or_else(|| {
            BundlebaseError::from(format!(
                "{}: No configuration set for /{}:{}",
                context, key.scope.name, key.key
            ))
        })
    }

    /// Compute the winning value for each (key, scope) pair across all sources.
    ///
    /// Returns a map of (key, scope) -> (winning_priority, winning_value).
    /// For each unique (key, scope) combination, the highest-priority source wins.
    fn compute_winners(inner: &ConfigInner) -> HashMap<(String, Scope), (u8, String)> {
        let mut winners: HashMap<(String, Scope), (u8, String)> = HashMap::new();

        for (source, entries) in &inner.entries {
            let priority = source.priority();
            for entry in entries {
                let scope = Scope::new(&entry.scope);
                let map_key = (entry.key.clone(), scope);
                winners
                    .entry(map_key)
                    .and_modify(|(p, v)| {
                        if priority >= *p {
                            *p = priority;
                            *v = entry.value.clone();
                        }
                    })
                    .or_insert((priority, entry.value.clone()));
            }
        }

        winners
    }

    /// Populate the active cache from the current entries.
    ///
    /// Groups winning entries by key, sorted by scope length descending
    /// (longest prefix first, global default last).
    fn populate_active_cache(inner: &mut ConfigInner) {
        let winners = Self::compute_winners(inner);
        let mut cache: HashMap<String, Vec<ActiveEntry>> = HashMap::new();

        for ((key, scope), (_priority, value)) in &winners {
            cache.entry(key.clone()).or_default().push(ActiveEntry {
                scope: scope.clone(),
                value: value.clone(),
            });
        }

        // Sort each key's entries by scope length descending (longest first, "/" last)
        for entries in cache.values_mut() {
            entries.sort_by(|a, b| b.scope.as_str().len().cmp(&a.scope.as_str().len()));
        }

        inner.active_cache = Some(cache);
        inner.winners_cache = Some(winners);
    }

    /// Look up the winning value for a key+scope from the pre-computed active cache.
    ///
    /// Iterates pre-sorted entries (longest scope first), returns the first
    /// entry whose scope matches the query scope (boundary-aware prefix matching)
    /// AND whose stored scope is compatible with the key's required `ConfigScope`.
    fn lookup_active(
        cache: &HashMap<String, Vec<ActiveEntry>>,
        key: &ConfigKey,
        scope: &Scope,
    ) -> Option<String> {
        let entries = cache.get(key.key)?;
        for entry in entries {
            if entry.scope.matches(scope) && key.scope.matches(&entry.scope) {
                return Some(entry.value.clone());
            }
        }
        None
    }

    /// Returns all config entries from all layers, with `active` flags
    /// marking which entry wins for each (key, scope) pair.
    ///
    /// `specs` is used to determine which keys are secure.
    pub fn all_values(&self, specs: &[ConfigKey]) -> Vec<ConfigValueDetails> {
        self.ensure_env_cache();

        // Fast path: check if caches are populated with read lock
        {
            let inner = self.inner.read();
            if let Some(winners) = &inner.winners_cache {
                return Self::build_all_values(&inner, winners, specs);
            }
        }

        // Slow path: populate caches with write lock
        let mut inner = self.inner.write();
        if inner.winners_cache.is_none() {
            Self::populate_active_cache(&mut inner);
        }

        let winners = inner.winners_cache.as_ref().expect("just populated");
        Self::build_all_values(&inner, winners, specs)
    }

    /// Build the full list of ConfigValueDetails from entries and pre-computed winners.
    fn build_all_values(
        inner: &ConfigInner,
        winners: &HashMap<(String, Scope), (u8, String)>,
        specs: &[ConfigKey],
    ) -> Vec<ConfigValueDetails> {
        let mut result = Vec::new();
        for (source, entries) in &inner.entries {
            let priority = source.priority();
            for entry in entries {
                let scope = Scope::new(&entry.scope);
                let winner_key = (entry.key.clone(), scope.clone());
                let is_active = winners
                    .get(&winner_key)
                    .map_or(false, |(p, _)| *p == priority);
                result.push(ConfigValueDetails {
                    key: entry.key.clone(),
                    value: entry.value.clone(),
                    scope,
                    source: source.clone(),
                    active: is_active,
                    secure: ConfigKey::is_key_secure(&entry.key, specs),
                });
            }
        }

        // Append synthetic default entries for keys that have defaults
        for spec in specs {
            if let Some(description) = spec.default_description() {
                let scope = Scope::new(&format!("/{}", spec.scope.name));
                let winner_key = (spec.key.to_string(), scope.clone());
                let is_active = !winners.contains_key(&winner_key) && spec.resolve_default().is_some();
                result.push(ConfigValueDetails {
                    key: spec.key.to_string(),
                    value: description.to_string(),
                    scope,
                    source: ConfigSource::Default,
                    active: is_active,
                    secure: spec.secure,
                });
            }
        }

        result
    }

    /// Returns only the winning (active) config entries.
    pub fn values(&self, specs: &[ConfigKey]) -> Vec<ConfigValueDetails> {
        self.all_values(specs)
            .into_iter()
            .filter(|e| e.active)
            .collect()
    }

    /// Extract Passed-source entries for transfer to a new BundleConfig (e.g., for views).
    /// Returns (key, value, scope) tuples where scope is the raw stored string.
    pub fn passed_entries(&self) -> Vec<(String, String, Scope)> {
        let inner = self.inner.read();
        let mut result = Vec::new();
        if let Some(entries) = inner.entries.get(&ConfigSource::Passed) {
            for entry in entries {
                result.push((
                    entry.key.clone(),
                    entry.value.clone(),
                    Scope::new(&entry.scope),
                ));
            }
        }
        result
    }

    /// Create `BundleConfig` from a nested `HashMap` (e.g., from Python dict).
    ///
    /// All values must be nested under a scope path (e.g., `{"s3://": {"region": "us-west-2"}}`).
    /// Flat top-level keys are rejected.
    pub fn from_map(
        map: HashMap<String, Value>,
        specs: &[ConfigKey],
    ) -> Result<Self, BundlebaseError> {
        let config = Self::new();

        for (key, value) in map {
            if Self::is_scope_key(&key) {
                // Scope-specific override — resolve path to scope
                let scope = Scope::from_path(&key)?;
                let scope_config = value.as_object().ok_or_else(|| {
                    BundlebaseError::from(format!("Scope key '{}' must have object value", key))
                })?;

                for (inner_key, inner_value) in scope_config {
                    let inner_str = inner_value.as_str().ok_or_else(|| {
                        BundlebaseError::from("Config value must be string".to_string())
                    })?;
                    let _spec = specs
                        .iter()
                        .find(|s| s.key == inner_key && s.scope.matches(&scope))
                        .ok_or_else(|| {
                            BundlebaseError::from(format!(
                                "Unknown config key '{}' for scope '{}'",
                                inner_key, scope
                            ))
                        })?;
                    config.set(inner_key, inner_str, &scope, ConfigSource::Passed);
                }
            } else {
                return Err(format!(
                    "Config key '{}' must be nested under a scope path. Example: {{\"s3://\": {{\"{}\": \"value\"}}}}",
                    key, key
                ).into());
            }
        }

        Ok(config)
    }

    /// Check if a key looks like a scope path (contains "://")
    fn is_scope_key(key: &str) -> bool {
        key.contains("://")
    }

    /// Ensure env vars are loaded into entries[Env]. Reads BB_* env vars on first call.
    ///
    /// Env var patterns (suffix after `BB_` is split on `__`):
    /// - `BB_KEY` -> global scope `/`, key = `key`
    /// - `BB_S3__REGION` -> scope `/s3`, key = `region`
    /// - `BB_S3__MY_BUCKET__KEY` -> scope `/s3/my_bucket`, key = `key`
    fn ensure_env_cache(&self) {
        // Fast path: check with read lock
        {
            let inner = self.inner.read();
            if inner.env_loaded {
                return;
            }
        }

        // Slow path: load env vars with write lock
        let mut inner = self.inner.write();
        // Double-check after acquiring write lock
        if inner.env_loaded {
            return;
        }

        let mut env_entries = Vec::new();

        for (raw_key, value) in std::env::vars() {
            let Some(suffix) = raw_key.strip_prefix("BB_") else {
                continue;
            };

            let parts: Vec<&str> = suffix.split("__").collect();
            if parts.len() == 1 {
                // BB_KEY -> global
                env_entries.push(ConfigValue {
                    key: suffix.to_lowercase(),
                    value,
                    scope: "/".to_string(),
                });
            } else {
                // BB_A__B__...__KEY -> last = key, rest joined with "/" = scope
                let key = parts.last().expect("split always returns at least one element").to_lowercase();
                let scope_parts: Vec<String> = parts[..parts.len() - 1]
                    .iter()
                    .map(|p| p.to_lowercase())
                    .collect();
                let scope = format!("/{}", scope_parts.join("/"));
                env_entries.push(ConfigValue {
                    key,
                    value,
                    scope,
                });
            }
        }

        inner.entries.insert(ConfigSource::Env, env_entries);
        inner.env_loaded = true;
        inner.active_cache = None;
        inner.winners_cache = None;
    }

    /// Merge another config's entries into this one. The other config's entries
    /// are added with their original sources. Entries from `other` take priority
    /// over entries in `self` with the same key+scope+source.
    pub fn merge(&self, other: &BundleConfig) {
        let other_inner = other.inner.read();
        let mut self_inner = self.inner.write();

        for (source, other_entries) in &other_inner.entries {
            let self_entries = self_inner.entries.entry(source.clone()).or_default();
            for entry in other_entries {
                // Remove any existing entry with same key+scope
                self_entries.retain(|e| !(e.key == entry.key && e.scope == entry.scope));
                self_entries.push(entry.clone());
            }
        }

        self_inner.active_cache = None;
        self_inner.winners_cache = None;
    }

    /// Absorb all entries from a `PassedBundleConfig` as `ConfigSource::Passed`
    /// entries into this config's internal storage.
    pub fn merge_passed(&self, passed: &PassedBundleConfig) {
        for (key, value) in &passed.defaults {
            self.set(key, value, &Scope::global(), ConfigSource::Passed);
        }
        for (scope, entries) in &passed.scoped {
            for (key, value) in entries {
                self.set(key, value, scope, ConfigSource::Passed);
            }
        }
    }

    /// Extract all `ConfigSource::Passed` entries back out into a
    /// `PassedBundleConfig`.
    pub fn extract_passed(&self) -> PassedBundleConfig {
        let inner = self.inner.read();
        let mut passed = PassedBundleConfig::new();
        if let Some(entries) = inner.entries.get(&ConfigSource::Passed) {
            for entry in entries {
                let scope = Scope::new(&entry.scope);
                passed.set(&entry.key, &entry.value, &scope);
            }
        }
        passed
    }
}

impl Default for BundleConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BundleConfig {
    fn clone(&self) -> Self {
        let inner = self.inner.read();
        let new_inner = ConfigInner {
            entries: inner.entries.clone(),
            env_loaded: inner.env_loaded,
            active_cache: None,
            winners_cache: None,
        };
        Self {
            inner: RwLock::new(new_inner),
        }
    }
}

/// Wire format for reading/writing config from manifests.
///
/// Used only for serialization — not used at runtime.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SerializedConfig {
    #[serde(default)]
    pub defaults: HashMap<String, String>,
    #[serde(default)]
    pub scope_overrides: HashMap<String, HashMap<String, String>>,
}

impl SerializedConfig {
    /// Convert from a BundleConfig, extracting only Stored entries.
    pub fn from_bundle_config(config: &BundleConfig) -> Self {
        let inner = config.inner.read();
        let mut defaults = HashMap::new();
        let mut scope_overrides: HashMap<String, HashMap<String, String>> = HashMap::new();

        if let Some(entries) = inner.entries.get(&ConfigSource::Stored) {
            for entry in entries {
                let scope = Scope::new(&entry.scope);
                if scope.is_global() {
                    defaults.insert(entry.key.clone(), entry.value.clone());
                } else {
                    scope_overrides
                        .entry(scope.as_str().to_string())
                        .or_default()
                        .insert(entry.key.clone(), entry.value.clone());
                }
            }
        }

        Self {
            defaults,
            scope_overrides,
        }
    }

    /// Load into a BundleConfig as Stored entries.
    pub fn into_bundle_config(&self, config: &BundleConfig) {
        for (key, value) in &self.defaults {
            config.set(key, value, &Scope::global(), ConfigSource::Stored);
        }
        for (scope_str, overrides) in &self.scope_overrides {
            let scope = Scope::new(scope_str);
            for (key, value) in overrides {
                config.set(key, value, &scope, ConfigSource::Stored);
            }
        }
    }
}

/// CommandResponse implementation for displaying config entries as a table.
impl CommandResponse for Vec<ConfigValueDetails> {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
            Field::new("scope", DataType::Utf8, false),
            Field::new("source", DataType::Utf8, false),
            Field::new("active", DataType::Boolean, false),
            Field::new("secure", DataType::Boolean, false),
        ]))
    }

    fn output_shape() -> OutputShape {
        OutputShape::Table
    }

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let keys: Vec<&str> = self.iter().map(|e| e.key.as_str()).collect();
        let values: Vec<String> = self
            .iter()
            .map(|e| {
                if e.secure {
                    "*****".to_string()
                } else {
                    e.value.clone()
                }
            })
            .collect();
        let value_refs: Vec<&str> = values.iter().map(|s| s.as_str()).collect();
        let scopes: Vec<String> = self.iter().map(|e| e.scope.as_str().to_string()).collect();
        let scope_refs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
        let sources: Vec<&str> = self.iter().map(|e| e.source.as_str()).collect();
        let actives: Vec<bool> = self.iter().map(|e| e.active).collect();
        let secures: Vec<bool> = self.iter().map(|e| e.secure).collect();

        let batch = RecordBatch::try_new(
            Self::schema(),
            vec![
                Arc::new(StringArray::from(keys)),
                Arc::new(StringArray::from(value_refs)),
                Arc::new(StringArray::from(scope_refs)),
                Arc::new(StringArray::from(sources)),
                Arc::new(BooleanArray::from(actives)),
                Arc::new(BooleanArray::from(secures)),
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;
        single_batch_stream(Self::schema(), batch)
    }

    impl_dyn_command_response!(Vec<ConfigValueDetails>);
}

/// CommandResponse implementation for displaying a single config entry as a dictionary.
impl CommandResponse for ConfigValueDetails {
    fn schema() -> SchemaRef {
        Vec::<ConfigValueDetails>::schema()
    }

    fn output_shape() -> OutputShape {
        OutputShape::Dictionary
    }

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        Box::new(vec![*self]).into_stream()
    }

    impl_dyn_command_response!(ConfigValueDetails);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test-only scopes
    const TEST_S3_SCOPE: ConfigScope = BundleConfig::register_scope("s3");
    const TEST_GCS_SCOPE: ConfigScope = BundleConfig::register_scope("gs");
    const TEST_AZURE_SCOPE: ConfigScope = BundleConfig::register_scope("azure");

    // Test-only config key constants
    const TEST_REGION: ConfigKey = TEST_S3_SCOPE.define("region");
    const TEST_ENDPOINT: ConfigKey = TEST_S3_SCOPE.define("endpoint");
    const TEST_ACCESS_KEY_ID: ConfigKey = TEST_S3_SCOPE.define("access_key_id");
    const TEST_KEY: ConfigKey = TEST_S3_SCOPE.define("key");
    const TEST_RUNTIME_KEY: ConfigKey = TEST_S3_SCOPE.define("runtime_key");
    const TEST_NEW_KEY: ConfigKey = TEST_S3_SCOPE.define("new_key");
    const TEST_STORED_KEY: ConfigKey = TEST_S3_SCOPE.define("stored_key");
    const TEST_TESTREGION1: ConfigKey = TEST_S3_SCOPE.define("testregion1");
    const TEST_TESTREGION2: ConfigKey = TEST_S3_SCOPE.define("testregion2");
    const TEST_TESTKEY3: ConfigKey = TEST_S3_SCOPE.define("testkey3");

    /// Test specs for validation tests.
    fn test_specs() -> Vec<ConfigKey> {
        vec![
            TEST_S3_SCOPE.define("region"),
            TEST_S3_SCOPE.define("access_key_id"),
            TEST_S3_SCOPE.define("endpoint"),
            TEST_S3_SCOPE.define("bucket"),
            TEST_S3_SCOPE.define("allow_http"),
            TEST_S3_SCOPE.define("skip_signature"),
            TEST_S3_SCOPE.define("virtual_hosted_style_request"),
            TEST_S3_SCOPE.define("imdsv1_fallback"),
            TEST_S3_SCOPE.define("metadata_endpoint"),
            TEST_S3_SCOPE.define("container_credentials_relative_uri"),
            TEST_S3_SCOPE.define("unsigned_payload"),
            TEST_S3_SCOPE.define("checksum_algorithm"),
            TEST_S3_SCOPE.define("copy_if_not_exists"),
            TEST_S3_SCOPE.define("conditional_put"),
            TEST_S3_SCOPE.define_secure("secret_access_key"),
            TEST_S3_SCOPE.define_secure("session_token"),
            TEST_S3_SCOPE.define_secure("token"),
            TEST_GCS_SCOPE.define("service_account_path"),
            TEST_GCS_SCOPE.define("application_credentials"),
            TEST_GCS_SCOPE.define_secure("service_account_key"),
            TEST_AZURE_SCOPE.define("account"),
            TEST_AZURE_SCOPE.define("container"),
            TEST_AZURE_SCOPE.define("client_id"),
            TEST_AZURE_SCOPE.define("tenant_id"),
            TEST_AZURE_SCOPE.define("authority_host"),
            TEST_AZURE_SCOPE.define("use_emulator"),
            TEST_AZURE_SCOPE.define_secure("access_key"),
            TEST_AZURE_SCOPE.define_secure("sas_token"),
            TEST_AZURE_SCOPE.define_secure("bearer_token"),
            TEST_AZURE_SCOPE.define_secure("client_secret"),
        ]
    }

    #[test]
    fn test_set_scoped_default() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::new("/s3"), ConfigSource::Stored);
        assert_eq!(config.get(&Scope::from_path("s3://bucket/file").unwrap(), &TEST_REGION), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_set_scoped_override() {
        let config = BundleConfig::new();
        config.set("endpoint", "http://localhost:9000", &Scope::from_path("s3://test/").unwrap(), ConfigSource::Stored);

        assert_eq!(
            config.get(&Scope::from_path("s3://test/file").unwrap(), &TEST_ENDPOINT),
            Some("http://localhost:9000".to_string())
        );
    }

    #[test]
    fn test_get_defaults_with_path() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::new("/s3"), ConfigSource::Stored);

        assert_eq!(config.get(&Scope::from_path("s3://my-bucket/path/to/file").unwrap(), &TEST_REGION), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_get_with_scoped_override() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::new("/s3"), ConfigSource::Stored);
        config.set("region", "us-east-1", &Scope::from_path("s3://special-bucket/").unwrap(), ConfigSource::Stored);

        assert_eq!(config.get(&Scope::from_path("s3://my-bucket/file").unwrap(), &TEST_REGION), Some("us-west-2".to_string()));
        assert_eq!(config.get(&Scope::from_path("s3://special-bucket/file").unwrap(), &TEST_REGION), Some("us-east-1".to_string()));
    }

    #[test]
    fn test_global_scope_filtered_out() {
        // A key with scope=s3 should NOT match entries stored at global scope
        let config = BundleConfig::new();
        config.set("region", "global-value", &Scope::global(), ConfigSource::Stored);
        assert_eq!(config.get(&Scope::from_path("s3://bucket/file").unwrap(), &TEST_REGION), None);
    }

    #[test]
    fn test_longest_prefix_matching() {
        let config = BundleConfig::new();
        config.set("endpoint", "default", &Scope::from_path("s3://bucket/").unwrap(), ConfigSource::Stored);
        config.set("endpoint", "specific", &Scope::from_path("s3://bucket/subfolder/").unwrap(), ConfigSource::Stored);

        // Should match the longer prefix
        assert_eq!(config.get(&Scope::from_path("s3://bucket/subfolder/file").unwrap(), &TEST_ENDPOINT), Some("specific".to_string()));

        // Should match the shorter prefix
        assert_eq!(config.get(&Scope::from_path("s3://bucket/otherpath/file").unwrap(), &TEST_ENDPOINT), Some("default".to_string()));
    }

    #[test]
    fn test_is_scope_key() {
        assert!(BundleConfig::is_scope_key("s3://bucket/"));
        assert!(BundleConfig::is_scope_key("gs://bucket/"));
        assert!(!BundleConfig::is_scope_key("region"));
        assert!(!BundleConfig::is_scope_key("access_key_id"));
    }

    #[test]
    fn test_validate_key_valid() {
        let specs = test_specs();
        assert!(ConfigKey::validate_key("access_key_id", &specs).is_ok());
        assert!(ConfigKey::validate_key("region", &specs).is_ok());
        assert!(ConfigKey::validate_key("service_account_key", &specs).is_ok());
    }

    #[test]
    fn test_validate_key_invalid() {
        let specs = test_specs();
        let result = ConfigKey::validate_key("invalid_key", &specs);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid config key 'invalid_key'"));
    }

    #[test]
    fn test_is_key_valid() {
        let specs = test_specs();
        assert!(ConfigKey::is_key_valid("region", &specs));
        assert!(ConfigKey::is_key_valid("secret_access_key", &specs));
        assert!(!ConfigKey::is_key_valid("nonexistent_key", &specs));
    }

    #[test]
    fn test_from_map_validates_scope_keyed() {
        let specs = test_specs();
        // Scope-keyed entry should succeed
        let mut map = HashMap::new();
        let mut inner = serde_json::Map::new();
        inner.insert("region".to_string(), Value::String("us-west-2".to_string()));
        map.insert("s3://".to_string(), Value::Object(inner));
        let result = BundleConfig::from_map(map, &specs);
        assert!(result.is_ok());
        let config = result.expect("from_map should succeed");
        assert_eq!(config.get(&Scope::from_path("s3://bucket/file").unwrap(), &TEST_REGION), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_from_map_rejects_flat_keys() {
        let specs = test_specs();
        let mut map = HashMap::new();
        map.insert(
            "region".to_string(),
            Value::String("us-west-2".to_string()),
        );
        let result = BundleConfig::from_map(map, &specs);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("must be nested under a scope path"), "Unexpected error: {}", err);
    }

    #[test]
    fn test_from_map_rejects_unknown_keys_in_scope() {
        let specs = test_specs();
        let mut map = HashMap::new();
        let mut inner = serde_json::Map::new();
        inner.insert("custom_setting".to_string(), Value::String("value".to_string()));
        map.insert("s3://".to_string(), Value::Object(inner));
        let result = BundleConfig::from_map(map, &specs);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown config key 'custom_setting'"));
    }

    #[test]
    fn test_merge() {
        let config1 = BundleConfig::new();
        config1.set("region", "us-west-2", &Scope::new("/s3"), ConfigSource::Stored);
        config1.set("endpoint", "old", &Scope::from_path("s3://bucket/").unwrap(), ConfigSource::Stored);

        let config2 = BundleConfig::new();
        config2.set("region", "us-east-1", &Scope::new("/s3"), ConfigSource::Stored);
        config2.set("access_key_id", "KEY123", &Scope::new("/s3"), ConfigSource::Stored);

        config1.merge(&config2);

        // config2 should override config1 for same key+scope+source
        assert_eq!(config1.get(&Scope::from_path("s3://bucket/file").unwrap(), &TEST_REGION), Some("us-east-1".to_string()));
        assert_eq!(config1.get(&Scope::from_path("s3://bucket/file").unwrap(), &TEST_ACCESS_KEY_ID), Some("KEY123".to_string()));
        assert_eq!(config1.get(&Scope::from_path("s3://bucket/file").unwrap(), &TEST_ENDPOINT), Some("old".to_string()));
    }

    // from_env tests use unique env var names to avoid conflicts between parallel tests
    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_from_env_global_default_filtered_by_scope() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_TESTREGION1", "us-west-2");

        let config = BundleConfig::new();
        // Force env cache load
        config.ensure_env_cache();

        // BB_TESTREGION1 stores at global scope (/), but TEST_TESTREGION1
        // requires s3 scope, so it should NOT match
        assert_eq!(
            config.get(&Scope::global(), &TEST_TESTREGION1),
            None
        );
        // Also not visible for s3 lookups since entry is at /
        assert_eq!(
            config.get(&Scope::from_path("s3://bucket/").unwrap(), &TEST_TESTREGION1),
            None
        );
        std::env::remove_var("BB_TESTREGION1");
    }

    #[test]
    fn test_from_env_scoped() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3__TESTREGION2", "us-west-2");

        let config = BundleConfig::new();
        config.ensure_env_cache();
        // BB_S3__TESTREGION2 -> scope "/s3", key "testregion2"
        assert_eq!(
            config.get(&Scope::new("/s3"), &TEST_TESTREGION2),
            Some("us-west-2".to_string())
        );
        // Should also match via prefix matching on child paths
        assert_eq!(
            config.get(&Scope::new("/s3/bucket"), &TEST_TESTREGION2),
            Some("us-west-2".to_string())
        );
        std::env::remove_var("BB_S3__TESTREGION2");
    }

    #[test]
    fn test_from_env_multi_segment_scope() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3__MY_BUCKET__TESTKEY3", "value");

        let config = BundleConfig::new();
        config.ensure_env_cache();
        // BB_S3__MY_BUCKET__TESTKEY3 -> scope "/s3/my_bucket", key "testkey3"
        assert_eq!(
            config.get(&Scope::new("/s3/my_bucket"), &TEST_TESTKEY3),
            Some("value".to_string())
        );
        std::env::remove_var("BB_S3__MY_BUCKET__TESTKEY3");
    }

    #[test]
    fn test_from_env_empty() {
        // from_env with no BB_ vars should not crash
        let config = BundleConfig::new();
        config.ensure_env_cache();
        let _ = config;
    }

    #[test]
    fn test_values_empty() {
        let config = BundleConfig::new();
        assert!(config.values(&[]).is_empty());
        assert!(config.all_values(&[]).is_empty());
    }

    #[test]
    fn test_values_single_layer() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::new("/s3"), ConfigSource::Stored);
        config.set("endpoint", "http://minio", &Scope::from_path("s3://bucket/").unwrap(), ConfigSource::Stored);

        let values = config.values(&[]);
        assert_eq!(values.len(), 2);

        let region = values.iter().find(|e| e.key == "region").expect("region entry");
        assert_eq!(region.value, "us-west-2");
        assert_eq!(region.scope, Scope::new("/s3"));
        assert_eq!(region.source, ConfigSource::Stored);
        assert!(region.active);

        let endpoint = values.iter().find(|e| e.key == "endpoint").expect("endpoint entry");
        assert_eq!(endpoint.value, "http://minio");
        assert_eq!(endpoint.scope, Scope::from_path("s3://bucket/").unwrap());
        assert!(endpoint.active);
    }

    #[test]
    fn test_all_values_multiple_layers() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::new("/s3"), ConfigSource::Stored);
        config.set("region", "us-east-1", &Scope::new("/s3"), ConfigSource::Runtime);

        let all = config.all_values(&[]);
        assert_eq!(all.len(), 2);

        let stored_entry = all.iter().find(|e| e.source == ConfigSource::Stored).expect("stored entry");
        assert_eq!(stored_entry.value, "us-west-2");
        assert!(!stored_entry.active, "stored should be overridden");

        let runtime_entry = all.iter().find(|e| e.source == ConfigSource::Runtime).expect("runtime entry");
        assert_eq!(runtime_entry.value, "us-east-1");
        assert!(runtime_entry.active, "runtime should win");

        // values() should only return the winner
        let active = config.values(&[]);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].value, "us-east-1");
        assert_eq!(active[0].source, ConfigSource::Runtime);
    }

    #[test]
    fn test_all_values_scoped() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::from_path("s3://bucket/").unwrap(), ConfigSource::Stored);
        config.set("region", "eu-west-1", &Scope::from_path("s3://bucket/").unwrap(), ConfigSource::Passed);
        config.set("endpoint", "http://localhost", &Scope::new("/s3"), ConfigSource::Passed);

        let all = config.all_values(&[]);
        assert_eq!(all.len(), 3);

        // Scoped "region": stored is overridden, passed wins
        let stored_region = all.iter().find(|e| {
            e.key == "region" && e.source == ConfigSource::Stored
        }).expect("stored region");
        assert!(!stored_region.active);

        let passed_region = all.iter().find(|e| {
            e.key == "region" && e.source == ConfigSource::Passed
        }).expect("passed region");
        assert!(passed_region.active);
        assert_eq!(passed_region.scope, Scope::from_path("s3://bucket/").unwrap());

        // Scoped "endpoint": only in passed, so active
        let endpoint = all.iter().find(|e| e.key == "endpoint").expect("endpoint");
        assert!(endpoint.active);
        assert_eq!(endpoint.source, ConfigSource::Passed);
    }

    #[test]
    fn test_secure_flag_on_entries() {
        let specs = test_specs();

        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::from_path("s3://bucket/").unwrap(), ConfigSource::Stored);
        config.set("secret_access_key", "SECRETKEY", &Scope::from_path("s3://bucket/").unwrap(), ConfigSource::Stored);
        config.set("endpoint", "http://localhost", &Scope::new("/s3"), ConfigSource::Stored);

        let all = config.all_values(&specs);
        assert_eq!(all.len(), 3);

        let region = all.iter().find(|e| e.key == "region").expect("region");
        assert!(!region.secure);

        let secret = all.iter().find(|e| e.key == "secret_access_key").expect("secret");
        assert!(secret.secure);

        let endpoint = all.iter().find(|e| e.key == "endpoint").expect("endpoint");
        assert!(!endpoint.secure);
    }

    #[test]
    fn test_is_key_secure() {
        let specs = test_specs();

        // Secure keys
        assert!(ConfigKey::is_key_secure("secret_access_key", &specs));
        assert!(ConfigKey::is_key_secure("session_token", &specs));
        assert!(ConfigKey::is_key_secure("access_key", &specs));
        assert!(ConfigKey::is_key_secure("service_account_key", &specs));
        assert!(ConfigKey::is_key_secure("client_secret", &specs));

        // Non-secure keys
        assert!(!ConfigKey::is_key_secure("region", &specs));
        assert!(!ConfigKey::is_key_secure("account", &specs));
        assert!(!ConfigKey::is_key_secure("bucket", &specs));

        // Unknown key — not secure
        assert!(!ConfigKey::is_key_secure("nonexistent_key", &specs));
    }

    #[test]
    fn test_passed_entries() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::new("/s3"), ConfigSource::Passed);
        config.set("endpoint", "http://minio", &Scope::from_path("s3://bucket/").unwrap(), ConfigSource::Passed);
        config.set("stored_key", "stored_value", &Scope::new("/s3"), ConfigSource::Stored);

        let passed = config.passed_entries();
        assert_eq!(passed.len(), 2);

        let region = passed.iter().find(|e| e.0 == "region").expect("region");
        assert_eq!(region.1, "us-west-2");
        assert_eq!(region.2, Scope::new("/s3"));

        let endpoint = passed.iter().find(|e| e.0 == "endpoint").expect("endpoint");
        assert_eq!(endpoint.1, "http://minio");
        assert_eq!(endpoint.2, Scope::from_path("s3://bucket/").unwrap());
    }

    #[test]
    fn test_reload_non_runtime() {
        let config1 = BundleConfig::new();
        config1.set("region", "us-west-2", &Scope::new("/s3"), ConfigSource::Stored);
        config1.set("runtime_key", "runtime_value", &Scope::new("/s3"), ConfigSource::Runtime);

        let config2 = BundleConfig::new();
        config2.set("region", "eu-west-1", &Scope::new("/s3"), ConfigSource::Stored);
        config2.set("new_key", "new_value", &Scope::new("/s3"), ConfigSource::Passed);

        config1.reload_non_runtime(&config2);

        // Runtime should be preserved
        assert_eq!(config1.get(&Scope::from_path("s3://bucket/").unwrap(), &TEST_RUNTIME_KEY), Some("runtime_value".to_string()));
        // Stored should come from config2
        assert_eq!(config1.get(&Scope::from_path("s3://bucket/").unwrap(), &TEST_REGION), Some("eu-west-1".to_string()));
        // Passed from config2 should be present
        assert_eq!(config1.get(&Scope::from_path("s3://bucket/").unwrap(), &TEST_NEW_KEY), Some("new_value".to_string()));
    }

    #[test]
    fn test_priority_ordering() {
        let config = BundleConfig::new();
        config.set("key", "stored", &Scope::new("/s3"), ConfigSource::Stored);
        config.set("key", "passed", &Scope::new("/s3"), ConfigSource::Passed);

        // Passed should win over Stored
        assert_eq!(config.get(&Scope::from_path("s3://bucket/").unwrap(), &TEST_KEY), Some("passed".to_string()));

        // Runtime should win over everything
        config.set("key", "runtime", &Scope::new("/s3"), ConfigSource::Runtime);
        assert_eq!(config.get(&Scope::from_path("s3://bucket/").unwrap(), &TEST_KEY), Some("runtime".to_string()));
    }

    #[test]
    fn test_serialized_config_roundtrip() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::new("/s3"), ConfigSource::Stored);
        config.set("endpoint", "http://localhost", &Scope::from_path("s3://test/").unwrap(), ConfigSource::Stored);

        let serialized = SerializedConfig::from_bundle_config(&config);
        assert_eq!(
            serialized.scope_overrides.get("/s3").and_then(|m| m.get("region")),
            Some(&"us-west-2".to_string())
        );
        assert_eq!(
            serialized.scope_overrides.get("/s3/test").and_then(|m| m.get("endpoint")),
            Some(&"http://localhost".to_string())
        );

        // Round-trip through YAML
        let yaml = serde_yaml_ng::to_string(&serialized).expect("serialize");
        let deserialized: SerializedConfig = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(serialized, deserialized);

        // Load back into a new BundleConfig
        let config2 = BundleConfig::new();
        deserialized.into_bundle_config(&config2);
        assert_eq!(config2.get(&Scope::from_path("s3://test/file").unwrap(), &TEST_REGION), Some("us-west-2".to_string()));
        assert_eq!(config2.get(&Scope::from_path("s3://test/file").unwrap(), &TEST_ENDPOINT), Some("http://localhost".to_string()));
    }

    #[test]
    fn test_longest_prefix_wins_over_source_priority() {
        let config = BundleConfig::new();
        config.set("region", "runtime-short", &Scope::from_path("s3://").unwrap(), ConfigSource::Runtime);
        config.set("region", "stored-long", &Scope::from_path("s3://bucket/").unwrap(), ConfigSource::Stored);

        // Longer prefix in Stored beats shorter prefix in Runtime
        assert_eq!(
            config.get(&Scope::from_path("s3://bucket/file").unwrap(), &TEST_REGION),
            Some("stored-long".to_string())
        );
        // Path that only matches the short prefix → Runtime wins
        assert_eq!(
            config.get(&Scope::from_path("s3://other/file").unwrap(), &TEST_REGION),
            Some("runtime-short".to_string())
        );
    }

    // ── ConfigScope tests ────────────────────────────────────────────

    #[test]
    fn test_config_scope_matches_exact() {
        let scope = BundleConfig::register_scope("s3");
        assert!(scope.matches(&Scope::new("/s3")));
    }

    #[test]
    fn test_config_scope_matches_child() {
        let scope = BundleConfig::register_scope("s3");
        assert!(scope.matches(&Scope::new("/s3/bucket")));
        assert!(scope.matches(&Scope::new("/s3/bucket/path")));
    }

    #[test]
    fn test_config_scope_rejects_global() {
        let scope = BundleConfig::register_scope("s3");
        assert!(!scope.matches(&Scope::global()));
    }

    #[test]
    fn test_config_scope_rejects_different_provider() {
        let scope = BundleConfig::register_scope("s3");
        assert!(!scope.matches(&Scope::new("/gs/bucket")));
        assert!(!scope.matches(&Scope::new("/azure/container")));
    }

    #[test]
    fn test_config_scope_rejects_partial_prefix() {
        let scope = BundleConfig::register_scope("s3");
        // "/s3x" should NOT match scope "s3"
        assert!(!scope.matches(&Scope::new("/s3x")));
    }

    #[test]
    fn test_all_scopes() {
        let scopes = BundleConfig::all_scopes();
        let names: Vec<&str> = scopes.iter().map(|s| s.name).collect();
        assert!(names.contains(&"s3"));
        assert!(names.contains(&"gs"));
        assert!(names.contains(&"azure"));
        assert!(names.contains(&"ftp"));
        assert!(names.contains(&"sftp"));
        assert!(names.contains(&"kaggle"));
    }

    #[test]
    fn test_validate_key_scoped() {
        let specs = test_specs();
        // "region" is in S3 scope
        assert!(ConfigKey::validate_key_scoped("region", &Scope::new("/s3"), &specs).is_ok());
        assert!(ConfigKey::validate_key_scoped("region", &Scope::new("/s3/bucket"), &specs).is_ok());
        // "region" is NOT in GCS scope
        assert!(ConfigKey::validate_key_scoped("region", &Scope::new("/gs"), &specs).is_err());
        // "account" is in Azure scope
        assert!(ConfigKey::validate_key_scoped("account", &Scope::new("/azure"), &specs).is_ok());
        assert!(ConfigKey::validate_key_scoped("account", &Scope::new("/s3"), &specs).is_err());
    }

    // ── Path-to-scope conversion tests ─────────────────────────────────

    #[test]
    fn test_default_scope_from_path_matching_scheme() {
        let scope = BundleConfig::register_scope("s3");
        let result = default_scope_from_path(&scope, "s3://bucket/path");
        assert_eq!(result, Some(Scope::new("/s3/bucket/path")));
    }

    #[test]
    fn test_default_scope_from_path_non_matching_scheme() {
        let scope = BundleConfig::register_scope("s3");
        let result = default_scope_from_path(&scope, "gs://bucket/path");
        assert_eq!(result, None);
    }

    #[test]
    fn test_default_scope_from_path_from_pathd() {
        let scope = BundleConfig::register_scope("s3");
        // Already-from_pathd paths like /s3/bucket should also match
        assert_eq!(default_scope_from_path(&scope, "/s3/bucket"), Some(Scope::new("/s3/bucket")));
        assert_eq!(default_scope_from_path(&scope, "/s3"), Some(Scope::new("/s3")));
        // But not a different scope's from_pathd path
        assert_eq!(default_scope_from_path(&scope, "/gs/bucket"), None);
    }

    #[test]
    fn test_config_scope_from_path_delegates() {
        let scope = BundleConfig::register_scope("s3");
        assert_eq!(scope.from_path("s3://bucket"), Some(Scope::new("/s3/bucket")));
        assert_eq!(scope.from_path("gs://bucket"), None);
    }

    #[test]
    fn test_config_scope_with_custom_fn() {
        fn custom(_scope: &ConfigScope, path: &str) -> Option<Scope> {
            if path.starts_with("custom://") {
                Some(Scope::new("/custom/matched"))
            } else {
                None
            }
        }
        let scope = BundleConfig::register_scope("custom").with_from_path(custom);
        assert_eq!(scope.from_path("custom://anything"), Some(Scope::new("/custom/matched")));
        assert_eq!(scope.from_path("s3://bucket"), None);
    }

    #[test]
    fn test_scope_from_path_s3() {
        assert_eq!(Scope::from_path("s3://bucket/path").unwrap(), Scope::new("/s3/bucket/path"));
    }

    #[test]
    fn test_scope_from_path_unknown_errors() {
        let result = Scope::from_path("not-a-valid-scope");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown scope"), "Expected 'Unknown scope' in: {}", err);
    }

    // ── ConfigKey default value tests ─────────────────────────────────

    const TEST_DEFAULT_SCOPE: ConfigScope = BundleConfig::register_scope("testdef");
    const TEST_KEY_WITH_DEFAULT: ConfigKey = TEST_DEFAULT_SCOPE
        .define("base_url")
        .with_default("https://default.example.com");
    const TEST_KEY_NO_DEFAULT: ConfigKey = TEST_DEFAULT_SCOPE.define("region");

    #[test]
    fn test_with_default_const_context() {
        // Verify that with_default() works in const context and the value is accessible
        assert!(TEST_KEY_WITH_DEFAULT.default_value.is_some());
        assert_eq!(TEST_KEY_WITH_DEFAULT.default_value, Some("https://default.example.com"));
        assert!(TEST_KEY_NO_DEFAULT.default_value.is_none());
        assert!(TEST_KEY_NO_DEFAULT.default_fn.is_none());
    }

    #[test]
    fn test_get_returns_static_default_when_no_value_set() {
        let config = BundleConfig::new();
        assert_eq!(
            config.get(&Scope::new("/testdef"), &TEST_KEY_WITH_DEFAULT),
            Some("https://default.example.com".to_string())
        );
        // Also matches child scopes
        assert_eq!(
            config.get(&Scope::new("/testdef/sub"), &TEST_KEY_WITH_DEFAULT),
            Some("https://default.example.com".to_string())
        );
    }

    #[test]
    fn test_get_returns_none_for_incompatible_scope_even_with_default() {
        let config = BundleConfig::new();
        // A different scope should not return the default
        assert_eq!(
            config.get(&Scope::new("/other"), &TEST_KEY_WITH_DEFAULT),
            None
        );
        assert_eq!(
            config.get(&Scope::global(), &TEST_KEY_WITH_DEFAULT),
            None
        );
    }

    #[test]
    fn test_get_returns_explicit_value_over_default() {
        let config = BundleConfig::new();
        config.set("base_url", "https://custom.example.com", &Scope::new("/testdef"), ConfigSource::Stored);
        assert_eq!(
            config.get(&Scope::new("/testdef"), &TEST_KEY_WITH_DEFAULT),
            Some("https://custom.example.com".to_string())
        );
    }

    #[test]
    fn test_all_values_includes_default_entry() {
        let config = BundleConfig::new();
        let specs = &[TEST_KEY_WITH_DEFAULT, TEST_KEY_NO_DEFAULT];
        let all = config.all_values(specs);

        let default_entry = all.iter().find(|e| {
            e.key == "base_url" && e.source == ConfigSource::Default
        });
        assert!(default_entry.is_some(), "Expected a default entry for base_url");
        let entry = default_entry.expect("just checked");
        assert_eq!(entry.value, "https://default.example.com");
        assert_eq!(entry.scope, Scope::new("/testdef"));
        assert!(entry.active, "Default should be active when no other value is set");

        // Key without default should NOT have a default entry
        assert!(!all.iter().any(|e| e.key == "region" && e.source == ConfigSource::Default));
    }

    #[test]
    fn test_all_values_marks_default_inactive_when_overridden() {
        let config = BundleConfig::new();
        config.set("base_url", "https://override.example.com", &Scope::new("/testdef"), ConfigSource::Stored);

        let specs = &[TEST_KEY_WITH_DEFAULT];
        let all = config.all_values(specs);

        let default_entry = all.iter().find(|e| e.source == ConfigSource::Default).expect("default entry");
        assert!(!default_entry.active, "Default should be inactive when overridden");

        let stored_entry = all.iter().find(|e| e.source == ConfigSource::Stored).expect("stored entry");
        assert!(stored_entry.active, "Stored value should be active");
        assert_eq!(stored_entry.value, "https://override.example.com");
    }

    // ── ConfigKey default_fn tests ────────────────────────────────────

    fn test_default_fn_value() -> Option<String> {
        Some("dynamic_value".to_string())
    }

    fn test_default_fn_none() -> Option<String> {
        None
    }

    const TEST_FN_SCOPE: ConfigScope = BundleConfig::register_scope("testfn");
    const TEST_KEY_WITH_DEFAULT_FN: ConfigKey = TEST_FN_SCOPE
        .define("dynamic_key")
        .with_default_fn("test source", test_default_fn_value);
    const TEST_KEY_WITH_DEFAULT_FN_NONE: ConfigKey = TEST_FN_SCOPE
        .define("none_key")
        .with_default_fn("test source", test_default_fn_none);

    #[test]
    fn test_with_default_fn_const_context() {
        assert!(TEST_KEY_WITH_DEFAULT_FN.default_fn.is_some());
        let (desc, _) = TEST_KEY_WITH_DEFAULT_FN.default_fn.expect("just checked");
        assert_eq!(desc, "test source");
    }

    #[test]
    fn test_get_returns_default_fn_value() {
        let config = BundleConfig::new();
        assert_eq!(
            config.get(&Scope::new("/testfn"), &TEST_KEY_WITH_DEFAULT_FN),
            Some("dynamic_value".to_string())
        );
        assert_eq!(
            config.get(&Scope::new("/testfn/sub"), &TEST_KEY_WITH_DEFAULT_FN),
            Some("dynamic_value".to_string())
        );
    }

    #[test]
    fn test_get_returns_none_when_default_fn_returns_none() {
        let config = BundleConfig::new();
        assert_eq!(
            config.get(&Scope::new("/testfn"), &TEST_KEY_WITH_DEFAULT_FN_NONE),
            None
        );
    }

    #[test]
    fn test_get_returns_none_for_incompatible_scope_with_default_fn() {
        let config = BundleConfig::new();
        assert_eq!(
            config.get(&Scope::new("/other"), &TEST_KEY_WITH_DEFAULT_FN),
            None
        );
        assert_eq!(
            config.get(&Scope::global(), &TEST_KEY_WITH_DEFAULT_FN),
            None
        );
    }

    #[test]
    fn test_default_fn_takes_priority_over_default_value() {
        // When both default_value and default_fn are set, default_fn wins
        const KEY_BOTH: ConfigKey = TEST_FN_SCOPE
            .define("both_key")
            .with_default("static_value")
            .with_default_fn("test source", test_default_fn_value);

        let config = BundleConfig::new();
        assert_eq!(
            config.get(&Scope::new("/testfn"), &KEY_BOTH),
            Some("dynamic_value".to_string())
        );
    }

    #[test]
    fn test_default_value_used_when_no_default_fn() {
        const KEY_STATIC_ONLY: ConfigKey = TEST_FN_SCOPE
            .define("static_only")
            .with_default("static_value");

        let config = BundleConfig::new();
        assert_eq!(
            config.get(&Scope::new("/testfn"), &KEY_STATIC_ONLY),
            Some("static_value".to_string())
        );
    }

    #[test]
    fn test_get_returns_explicit_value_over_default_fn() {
        let config = BundleConfig::new();
        config.set("dynamic_key", "explicit", &Scope::new("/testfn"), ConfigSource::Stored);
        assert_eq!(
            config.get(&Scope::new("/testfn"), &TEST_KEY_WITH_DEFAULT_FN),
            Some("explicit".to_string())
        );
    }

    #[test]
    fn test_all_values_includes_default_fn_entry() {
        let config = BundleConfig::new();
        let specs = &[TEST_KEY_WITH_DEFAULT_FN];
        let all = config.all_values(specs);

        let fn_default_entry = all.iter().find(|e| {
            e.key == "dynamic_key" && e.source == ConfigSource::Default
        });
        assert!(fn_default_entry.is_some(), "Expected a default entry for dynamic_key");
        let entry = fn_default_entry.expect("just checked");
        assert_eq!(entry.value, "test source");
        assert_eq!(entry.scope, Scope::new("/testfn"));
        assert!(entry.active, "Dynamic default should be active when no other value is set and fn returns Some");
    }

    #[test]
    fn test_all_values_default_fn_inactive_when_fn_returns_none() {
        let config = BundleConfig::new();
        let specs = &[TEST_KEY_WITH_DEFAULT_FN_NONE];
        let all = config.all_values(specs);

        let fn_default_entry = all.iter().find(|e| {
            e.key == "none_key" && e.source == ConfigSource::Default
        });
        assert!(fn_default_entry.is_some(), "Expected a default entry for none_key");
        let entry = fn_default_entry.expect("just checked");
        assert!(!entry.active, "Dynamic default should be inactive when fn returns None");
    }

    #[test]
    fn test_all_values_default_fn_inactive_when_overridden() {
        let config = BundleConfig::new();
        config.set("dynamic_key", "override", &Scope::new("/testfn"), ConfigSource::Stored);

        let specs = &[TEST_KEY_WITH_DEFAULT_FN];
        let all = config.all_values(specs);

        let fn_default_entry = all.iter().find(|e| {
            e.key == "dynamic_key" && e.source == ConfigSource::Default
        }).expect("default entry");
        assert!(!fn_default_entry.active, "Dynamic default should be inactive when overridden");

        let stored_entry = all.iter().find(|e| e.source == ConfigSource::Stored).expect("stored entry");
        assert!(stored_entry.active);
        assert_eq!(stored_entry.value, "override");
    }

}
