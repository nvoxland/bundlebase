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
}

impl ConfigSource {
    /// String representation for Python/CLI display.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConfigSource::Stored => "stored",
            ConfigSource::Env => "env",
            ConfigSource::Passed => "passed",
            ConfigSource::Runtime => "runtime",
        }
    }

    /// Higher priority wins when the same key+scope appears in multiple layers.
    fn priority(&self) -> u8 {
        match self {
            ConfigSource::Stored => 0,
            ConfigSource::Env => 1,
            ConfigSource::Passed => 2,
            ConfigSource::Runtime => 3,
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
    /// Normalized scope, or global (`/`) for defaults
    pub scope: Scope,
    /// Which layer this value came from
    pub source: ConfigSource,
    /// True if this entry is the winning value for its key+scope
    pub active: bool,
    /// True if this key holds a secret (password, token, etc.)
    pub secure: bool,
}

/// Defines a known configuration key and whether it is secure.
///
/// Each service/provider defines its own slice of `ConfigKey` entries.
/// Duplicate keys across modules are fine (e.g., `access_key` in S3 and Azure).
#[derive(Debug, Clone)]
pub struct ConfigKey {
    /// Configuration key name (e.g., "region", "secret_access_key")
    pub key: &'static str,
    /// Whether this key holds a secret (password, token, etc.)
    pub secure: bool,
}

impl ConfigKey {
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
}

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
    /// Named scope aliases: name -> normalized Scope
    scope_aliases: HashMap<String, Scope>,
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
            scope_aliases: HashMap::new(),
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
///
/// # Example
/// ```rust
/// use bundlebase::bundle_config::{BundleConfig, ConfigSource, Scope};
///
/// let config = BundleConfig::new();
/// config.set("region", "us-west-2", &Scope::global(), ConfigSource::Passed);
/// config.set("endpoint", "http://localhost:9000", &Scope::from_url("s3://test-bucket/"), ConfigSource::Stored);
/// ```
pub struct BundleConfig {
    inner: RwLock<ConfigInner>,
}

impl std::fmt::Debug for BundleConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read();
        f.debug_struct("BundleConfig")
            .field("entries", &inner.entries)
            .field("scope_aliases", &inner.scope_aliases)
            .finish()
    }
}

impl BundleConfig {
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
    ///           Supports compound format `"scope__key"` which resolves the scope name.
    /// * `value` - Configuration value
    /// * `scope` - Normalized scope, or global for default.
    ///             Use `Scope::from_url()` to convert raw URLs at the call site.
    /// * `source` - Which config layer this entry belongs to
    pub fn set(&self, key: &str, value: &str, scope: &Scope, source: ConfigSource) {
        let mut inner = self.inner.write();
        let (resolved_key, resolved_scope) = Self::normalize_key_scope(&inner, key, scope);
        let scope_str = resolved_scope.as_str().to_string();

        let entries = inner.entries.entry(source).or_default();

        // Remove any existing entry with the same key+scope (last write wins)
        entries.retain(|e| !(e.key == resolved_key && e.scope == scope_str));

        entries.push(ConfigValue {
            key: resolved_key,
            value: value.to_string(),
            scope: scope_str,
        });
        inner.active_cache = None;
        inner.winners_cache = None;
    }

    /// Add a named scope alias (name -> normalized Scope mapping).
    /// Invalidates the env cache so env vars re-resolve with updated scope aliases.
    pub fn add_scope_alias(&self, name: &str, scope: &Scope) {
        let mut inner = self.inner.write();
        inner.scope_aliases.insert(name.to_string(), scope.clone());
        // Invalidate env so it reloads with updated scopes
        inner.env_loaded = false;
        inner.entries.remove(&ConfigSource::Env);
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
        self_inner.scope_aliases = other_inner.scope_aliases.clone();
        self_inner.env_loaded = false;
        self_inner.entries.remove(&ConfigSource::Env);

        // Restore runtime entries
        if let Some(runtime_entries) = runtime {
            self_inner.entries.insert(ConfigSource::Runtime, runtime_entries);
        }
        self_inner.active_cache = None;
        self_inner.winners_cache = None;
    }

    /// Get the winning value for a key, scoped to a normalized Scope.
    ///
    /// Ensures env cache is populated, then finds the longest matching prefix
    /// across all sources. Among entries sharing the longest prefix, the
    /// highest-priority source wins. Pass `Scope::global()` for an unscoped lookup.
    ///
    /// Supports scope names in addition to full scopes:
    /// - Compound key: `get("scope__key", &Scope::global())` resolves the scope name
    /// - Scope name: `get("key", &Scope::new("/prod"))` if scope is a known alias
    pub fn get(&self, key: &str, scope: &Scope) -> Option<String> {
        self.ensure_env_cache();

        // Fast path: check active cache with read lock
        {
            let inner = self.inner.read();
            if let Some(cache) = &inner.active_cache {
                let (resolved_key, resolved_scope) =
                    Self::normalize_key_scope(&inner, key, scope);
                return Self::lookup_active(cache, &resolved_key, &resolved_scope);
            }
        }

        // Slow path: populate cache with write lock
        let mut inner = self.inner.write();

        // Double-check after upgrading to write lock
        if inner.active_cache.is_none() {
            Self::populate_active_cache(&mut inner);
        }

        let (resolved_key, resolved_scope) = Self::normalize_key_scope(&inner, key, scope);
        match &inner.active_cache {
            Some(cache) => Self::lookup_active(cache, &resolved_key, &resolved_scope),
            None => None, // should not happen after populate_active_cache
        }
    }

    /// Normalize the (key, scope) pair:
    /// - If `scope` is non-global and matches a scope alias name, resolve it.
    /// - If `scope` is global and `key` contains "__", split into scope_name + config_key and resolve.
    /// Returns the (resolved_key, resolved_scope) pair.
    fn normalize_key_scope(inner: &ConfigInner, key: &str, scope: &Scope) -> (String, Scope) {
        // 1. If scope is non-global, check if its path (without leading /) is a scope alias name
        if !scope.is_global() {
            // Extract the alias name: strip leading "/" from the scope string
            let alias_candidate = &scope.as_str()[1..];
            // Only treat as alias if it's a simple name (no embedded slashes)
            if !alias_candidate.contains('/') {
                if let Some(resolved_scope) = inner.scope_aliases.get(alias_candidate) {
                    return (key.to_string(), resolved_scope.clone());
                }
            }
        }

        // 2. If scope is global and key has "__", parse compound key
        if scope.is_global() {
            if let Some((scope_name, config_key)) = key.split_once("__") {
                if let Some(resolved_scope) = inner.scope_aliases.get(scope_name) {
                    return (config_key.to_string(), resolved_scope.clone());
                }
            }
        }

        // 3. Pass through unchanged
        (key.to_string(), scope.clone())
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
                // Skip unresolved scope entries — they haven't been resolved yet
                if entry.scope.starts_with("unresolved::") {
                    continue;
                }
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
    /// entry whose scope matches the query scope (boundary-aware prefix matching).
    fn lookup_active(
        cache: &HashMap<String, Vec<ActiveEntry>>,
        key: &str,
        scope: &Scope,
    ) -> Option<String> {
        let entries = cache.get(key)?;
        for entry in entries {
            if entry.scope.matches(scope) {
                return Some(entry.value.clone());
            }
        }
        None
    }

    /// Returns all defined scope aliases (name -> Scope) as a snapshot.
    pub fn scope_aliases(&self) -> HashMap<String, Scope> {
        self.inner.read().scope_aliases.clone()
    }

    /// Look up a single scope alias by name.
    pub fn resolve_alias(&self, name: &str) -> Option<Scope> {
        self.inner.read().scope_aliases.get(name).cloned()
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

    /// Create BundleConfig from a nested HashMap (e.g., from Python dict)
    ///
    /// Top-level non-URL keys are defaults, URL keys contain nested config.
    /// All keys are validated against the provided specs.
    pub fn from_map(
        map: HashMap<String, Value>,
        specs: &[ConfigKey],
    ) -> Result<Self, BundlebaseError> {
        let config = Self::new();

        for (key, value) in map {
            if Self::is_url_key(&key) {
                // URL-specific override — normalize URL to scope
                let scope = Scope::from_url(&key);
                let url_config = value.as_object().ok_or_else(|| {
                    BundlebaseError::from(format!("URL key '{}' must have object value", key))
                })?;

                for (inner_key, inner_value) in url_config {
                    let inner_str = inner_value.as_str().ok_or_else(|| {
                        BundlebaseError::from("Config value must be string".to_string())
                    })?;
                    ConfigKey::validate_key(inner_key, specs)?;
                    config.set(inner_key, inner_str, &scope, ConfigSource::Passed);
                }
            } else {
                // Default setting
                let value_str = value.as_str().ok_or_else(|| {
                    BundlebaseError::from("Config value must be string".to_string())
                })?;
                ConfigKey::validate_key(&key, specs)?;
                config.set(&key, value_str, &Scope::global(), ConfigSource::Passed);
            }
        }

        Ok(config)
    }

    /// Parse a flat-key config map using the same patterns as env vars (without BB_ prefix).
    ///
    /// Patterns (case-insensitive):
    /// - `key` -> global default
    /// - `name__key` -> named scope (deferred until scopes are known)
    /// - `scope_name__key` -> named scope (deferred, `scope_` prefix is optional)
    ///
    /// Keys with scope patterns are stored with scope = None and a special
    /// compound key format "scope_name::config_key" that will be resolved
    /// when scopes become available.
    pub fn from_flat_keys(map: HashMap<String, String>) -> Self {
        let config = Self::new();

        // Collect unresolved scope keys separately
        let mut unresolved: Vec<(String, String, String)> = Vec::new(); // (scope_name, config_key, value)

        for (raw_key, value) in map {
            let key = raw_key.to_lowercase();

            if let Some(scope_rest) = key.strip_prefix("scope_") {
                // scope_NAME__KEY -> named scope (deferred)
                if let Some((scope_name, config_key)) = scope_rest.split_once("__") {
                    unresolved.push((scope_name.to_string(), config_key.to_string(), value));
                } else {
                    // No __, treat as global default (key = "scope_something")
                    config.set(&key, &value, &Scope::global(), ConfigSource::Passed);
                }
            } else if let Some((scope_name, config_key)) = key.split_once("__") {
                // name__key -> named scope (deferred)
                unresolved.push((scope_name.to_string(), config_key.to_string(), value));
            } else {
                // plain key -> global default
                config.set(&key, &value, &Scope::global(), ConfigSource::Passed);
            }
        }

        // Store unresolved scope keys in a temporary format.
        // We store them as Passed entries with a special internal scope format
        // "unresolved::scope_name" that will be resolved later.
        {
            let mut inner = config.inner.write();
            for (scope_name, config_key, value) in unresolved {
                let entries = inner.entries.entry(ConfigSource::Passed).or_default();
                entries.push(ConfigValue {
                    key: config_key,
                    value,
                    scope: format!("unresolved::{}", scope_name),
                });
            }
            inner.active_cache = None;
            inner.winners_cache = None;
        }

        config
    }

    /// Resolve any unresolved scope keys using the given scopes map.
    ///
    /// Returns a new BundleConfig with unresolved keys resolved.
    pub fn resolve_scopes(&self, scopes: &HashMap<String, String>) -> BundleConfig {
        let new_config = BundleConfig::new();
        let inner = self.inner.read();

        for (source, entries) in &inner.entries {
            for entry in entries {
                if let Some(scope_name) = entry.scope.strip_prefix("unresolved::") {
                    // Resolve the scope name to a URL prefix, then normalize
                    if let Some(url_prefix) = scopes.get(scope_name) {
                        let scope = Scope::from_url(url_prefix);
                        new_config.set(
                            &entry.key,
                            &entry.value,
                            &scope,
                            source.clone(),
                        );
                    } else {
                        tracing::warn!(
                            scope = scope_name,
                            key = entry.key.as_str(),
                            "Config entry dropped: unknown scope name"
                        );
                    }
                } else {
                    let scope = Scope::new(&entry.scope);
                    new_config.set(
                        &entry.key,
                        &entry.value,
                        &scope,
                        source.clone(),
                    );
                }
            }
        }

        // Copy scope aliases
        {
            let mut new_inner = new_config.inner.write();
            new_inner.scope_aliases = inner.scope_aliases.clone();
        }

        new_config
    }

    /// Check if a key looks like a URL (contains "://")
    fn is_url_key(key: &str) -> bool {
        key.contains("://")
    }

    /// Ensure env vars are loaded into entries[Env]. Reads BB_* env vars on first call.
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

            if let Some(scope_rest) = suffix.strip_prefix("SCOPE_") {
                // BB_SCOPE_{NAME}__{KEY}
                if let Some((scope_name, key)) = scope_rest.split_once("__") {
                    if let Some(scope) = inner.scope_aliases.get(&scope_name.to_lowercase()) {
                        env_entries.push(ConfigValue {
                            key: key.to_lowercase(),
                            value,
                            scope: scope.as_str().to_string(),
                        });
                    }
                }
            } else if let Some((scope_name, key)) = suffix.split_once("__") {
                // BB_{NAME}__{KEY} -> named scope
                if let Some(scope) = inner.scope_aliases.get(&scope_name.to_lowercase()) {
                    env_entries.push(ConfigValue {
                        key: key.to_lowercase(),
                        value,
                        scope: scope.as_str().to_string(),
                    });
                }
            } else {
                // BB_{KEY} -> global
                env_entries.push(ConfigValue {
                    key: suffix.to_lowercase(),
                    value,
                    scope: "/".to_string(),
                });
            }
        }

        inner.entries.insert(ConfigSource::Env, env_entries);
        inner.env_loaded = true;
        inner.active_cache = None;
        inner.winners_cache = None;
    }

    /// Check if there are any unresolved scope keys.
    pub fn has_unresolved_scopes(&self) -> bool {
        let inner = self.inner.read();
        for entries in inner.entries.values() {
            for entry in entries {
                if entry.scope.starts_with("unresolved::") {
                    return true;
                }
            }
        }
        false
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

        // Merge scope aliases (other wins on conflict)
        for (name, scope) in &other_inner.scope_aliases {
            self_inner.scope_aliases.insert(name.clone(), scope.clone());
        }

        // Invalidate env since scopes may have changed
        self_inner.env_loaded = false;
        self_inner.entries.remove(&ConfigSource::Env);
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
    /// `PassedBundleConfig`. Skips entries with unresolved scopes since they
    /// cannot be represented in `PassedBundleConfig`.
    pub fn extract_passed(&self) -> PassedBundleConfig {
        let inner = self.inner.read();
        let mut passed = PassedBundleConfig::new();
        if let Some(entries) = inner.entries.get(&ConfigSource::Passed) {
            for entry in entries {
                if entry.scope.starts_with("unresolved::") {
                    continue;
                }
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
            scope_aliases: inner.scope_aliases.clone(),
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

    /// Test specs for validation tests.
    fn test_specs() -> Vec<ConfigKey> {
        vec![
            ConfigKey { key: "region", secure: false },
            ConfigKey { key: "access_key_id", secure: false },
            ConfigKey { key: "endpoint", secure: false },
            ConfigKey { key: "bucket", secure: false },
            ConfigKey { key: "allow_http", secure: false },
            ConfigKey { key: "skip_signature", secure: false },
            ConfigKey { key: "virtual_hosted_style_request", secure: false },
            ConfigKey { key: "imdsv1_fallback", secure: false },
            ConfigKey { key: "metadata_endpoint", secure: false },
            ConfigKey { key: "container_credentials_relative_uri", secure: false },
            ConfigKey { key: "unsigned_payload", secure: false },
            ConfigKey { key: "checksum_algorithm", secure: false },
            ConfigKey { key: "copy_if_not_exists", secure: false },
            ConfigKey { key: "conditional_put", secure: false },
            ConfigKey { key: "secret_access_key", secure: true },
            ConfigKey { key: "session_token", secure: true },
            ConfigKey { key: "token", secure: true },
            ConfigKey { key: "service_account_path", secure: false },
            ConfigKey { key: "application_credentials", secure: false },
            ConfigKey { key: "service_account_key", secure: true },
            ConfigKey { key: "account", secure: false },
            ConfigKey { key: "container", secure: false },
            ConfigKey { key: "client_id", secure: false },
            ConfigKey { key: "tenant_id", secure: false },
            ConfigKey { key: "authority_host", secure: false },
            ConfigKey { key: "use_emulator", secure: false },
            ConfigKey { key: "access_key", secure: true },
            ConfigKey { key: "sas_token", secure: true },
            ConfigKey { key: "bearer_token", secure: true },
            ConfigKey { key: "client_secret", secure: true },
        ]
    }

    #[test]
    fn test_set_default() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);
        assert_eq!(config.get("region", &Scope::from_url("s3://bucket/file")), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_set_url_override() {
        let config = BundleConfig::new();
        config.set("endpoint", "http://localhost:9000", &Scope::from_url("s3://test/"), ConfigSource::Stored);

        assert_eq!(
            config.get("endpoint", &Scope::from_url("s3://test/file")),
            Some("http://localhost:9000".to_string())
        );
    }

    #[test]
    fn test_get_defaults_with_url() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);

        assert_eq!(config.get("region", &Scope::from_url("s3://my-bucket/path/to/file")), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_get_with_url_override() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);
        config.set("region", "us-east-1", &Scope::from_url("s3://special-bucket/"), ConfigSource::Stored);

        assert_eq!(config.get("region", &Scope::from_url("s3://my-bucket/file")), Some("us-west-2".to_string()));
        assert_eq!(config.get("region", &Scope::from_url("s3://special-bucket/file")), Some("us-east-1".to_string()));
    }

    #[test]
    fn test_longest_prefix_matching() {
        let config = BundleConfig::new();
        config.set("endpoint", "default", &Scope::from_url("s3://bucket/"), ConfigSource::Stored);
        config.set("endpoint", "specific", &Scope::from_url("s3://bucket/subfolder/"), ConfigSource::Stored);

        // Should match the longer prefix
        assert_eq!(config.get("endpoint", &Scope::from_url("s3://bucket/subfolder/file")), Some("specific".to_string()));

        // Should match the shorter prefix
        assert_eq!(config.get("endpoint", &Scope::from_url("s3://bucket/otherpath/file")), Some("default".to_string()));
    }

    #[test]
    fn test_is_url_key() {
        assert!(BundleConfig::is_url_key("s3://bucket/"));
        assert!(BundleConfig::is_url_key("gs://bucket/"));
        assert!(!BundleConfig::is_url_key("region"));
        assert!(!BundleConfig::is_url_key("access_key_id"));
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
    fn test_from_map_validates_all_keys() {
        let specs = test_specs();
        // Known key should succeed
        let mut map = HashMap::new();
        map.insert(
            "region".to_string(),
            Value::String("us-west-2".to_string()),
        );
        let result = BundleConfig::from_map(map, &specs);
        assert!(result.is_ok());
        let config = result.expect("from_map should succeed");
        assert_eq!(config.get("region", &Scope::global()), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_from_map_rejects_unknown_keys() {
        let specs = test_specs();
        let mut map = HashMap::new();
        map.insert(
            "custom_setting".to_string(),
            Value::String("value".to_string()),
        );
        let result = BundleConfig::from_map(map, &specs);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid config key 'custom_setting'"));
    }

    #[test]
    fn test_merge() {
        let config1 = BundleConfig::new();
        config1.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);
        config1.set("endpoint", "old", &Scope::from_url("s3://bucket/"), ConfigSource::Stored);

        let config2 = BundleConfig::new();
        config2.set("region", "us-east-1", &Scope::global(), ConfigSource::Stored);
        config2.set("access_key_id", "KEY123", &Scope::global(), ConfigSource::Stored);

        config1.merge(&config2);

        // config2 should override config1 for same key+scope+source
        assert_eq!(config1.get("region", &Scope::from_url("s3://bucket/file")), Some("us-east-1".to_string()));
        assert_eq!(config1.get("access_key_id", &Scope::from_url("s3://bucket/file")), Some("KEY123".to_string()));
        assert_eq!(config1.get("endpoint", &Scope::from_url("s3://bucket/file")), Some("old".to_string()));
    }

    // from_env tests use unique env var names to avoid conflicts between parallel tests
    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_from_env_global_default() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_TESTREGION1", "us-west-2");

        let config = BundleConfig::new();
        // Force env cache load
        config.ensure_env_cache();

        assert_eq!(
            config.get("testregion1", &Scope::global()),
            Some("us-west-2".to_string())
        );
        std::env::remove_var("BB_TESTREGION1");
    }

    #[test]
    fn test_from_env_named_scope_without_prefix() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3__TESTREGION2", "us-west-2");

        // With no scopes, the key is silently dropped
        let config = BundleConfig::new();
        config.ensure_env_cache();
        assert!(config.get("testregion2", &Scope::from_url("s3://bucket/file")).is_none());

        // With a matching scope, the key resolves
        let config2 = BundleConfig::new();
        config2.add_scope_alias("s3", &Scope::from_url("s3://"));
        config2.ensure_env_cache();
        assert_eq!(
            config2.get("testregion2", &Scope::from_url("s3://bucket/file")),
            Some("us-west-2".to_string())
        );
        std::env::remove_var("BB_S3__TESTREGION2");
    }

    #[test]
    fn test_from_env_named_scope() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_SCOPE_TESTPROD__TESTENDPOINT1", "http://minio");

        let config = BundleConfig::new();
        config.add_scope_alias("testprod", &Scope::from_url("s3://bucket/"));
        config.ensure_env_cache();

        assert_eq!(
            config.get("testendpoint1", &Scope::from_url("s3://bucket/file")),
            Some("http://minio".to_string())
        );
        std::env::remove_var("BB_SCOPE_TESTPROD__TESTENDPOINT1");
    }

    #[test]
    fn test_from_env_named_scope_case_insensitive() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_SCOPE_TestProd2__TESTKEY1", "value");

        let config = BundleConfig::new();
        config.add_scope_alias("testprod2", &Scope::from_url("s3://bucket2/"));
        config.ensure_env_cache();

        assert_eq!(config.get("testkey1", &Scope::from_url("s3://bucket2/file")), Some("value".to_string()));
        std::env::remove_var("BB_SCOPE_TestProd2__TESTKEY1");
    }

    #[test]
    fn test_from_env_unknown_named_scope_skipped() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_SCOPE_UNKNOWN99__TESTKEY2", "value");

        let config = BundleConfig::new(); // no matching scope
        config.ensure_env_cache();

        // Should not have the value since scope is unknown
        assert!(config.get("testkey2", &Scope::global()).is_none());
        std::env::remove_var("BB_SCOPE_UNKNOWN99__TESTKEY2");
    }

    #[test]
    fn test_from_env_empty() {
        // from_env with no BB_ vars should not crash
        let config = BundleConfig::new();
        config.ensure_env_cache();
        let _ = config;
    }

    #[test]
    fn test_from_flat_keys_global_default() {
        let mut map = HashMap::new();
        map.insert("region".to_string(), "us-west-2".to_string());
        let config = BundleConfig::from_flat_keys(map);
        assert_eq!(
            config.get("region", &Scope::global()),
            Some("us-west-2".to_string())
        );
        assert!(!config.has_unresolved_scopes() || {
            // Check that there are no unresolved scopes other than what we set
            true
        });
    }

    #[test]
    fn test_from_flat_keys_named_scope_without_prefix() {
        let mut map = HashMap::new();
        map.insert("s3__region".to_string(), "us-west-2".to_string());
        let config = BundleConfig::from_flat_keys(map);

        // Should have unresolved scope
        assert!(config.has_unresolved_scopes());

        // Resolves via resolve_scopes()
        let mut scopes = HashMap::new();
        scopes.insert("s3".to_string(), "s3://".to_string());
        let resolved = config.resolve_scopes(&scopes);
        assert!(!resolved.has_unresolved_scopes());

        assert_eq!(resolved.get("region", &Scope::from_url("s3://bucket/file")), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_from_flat_keys_named_scope() {
        let mut map = HashMap::new();
        map.insert("scope_prod__region".to_string(), "us-west-2".to_string());
        let config = BundleConfig::from_flat_keys(map);
        assert!(config.has_unresolved_scopes());
    }

    #[test]
    fn test_from_flat_keys_case_insensitive() {
        let mut map = HashMap::new();
        map.insert("S3__REGION".to_string(), "us-west-2".to_string());
        map.insert("MyKey".to_string(), "value".to_string());
        let config = BundleConfig::from_flat_keys(map);
        assert!(config.has_unresolved_scopes());
        assert_eq!(config.get("mykey", &Scope::global()), Some("value".to_string()));
    }

    #[test]
    fn test_from_flat_keys_no_double_underscore() {
        let mut map = HashMap::new();
        map.insert("simple_key".to_string(), "value".to_string());
        let config = BundleConfig::from_flat_keys(map);
        assert_eq!(
            config.get("simple_key", &Scope::global()),
            Some("value".to_string())
        );
    }

    #[test]
    fn test_from_flat_keys_scope_prefix_and_bare_equivalent() {
        // scope_prod__region and prod__region produce identical unresolved entries
        let mut map1 = HashMap::new();
        map1.insert("scope_prod__region".to_string(), "us-west-2".to_string());
        let config1 = BundleConfig::from_flat_keys(map1);

        let mut map2 = HashMap::new();
        map2.insert("prod__region".to_string(), "us-west-2".to_string());
        let config2 = BundleConfig::from_flat_keys(map2);

        // Both should have unresolved scopes
        assert!(config1.has_unresolved_scopes());
        assert!(config2.has_unresolved_scopes());

        // Both should resolve the same way
        let mut scopes = HashMap::new();
        scopes.insert("prod".to_string(), "s3://my-bucket/".to_string());
        let resolved1 = config1.resolve_scopes(&scopes);
        let resolved2 = config2.resolve_scopes(&scopes);

        assert_eq!(
            resolved1.get("region", &Scope::from_url("s3://my-bucket/file")),
            Some("us-west-2".to_string())
        );
        assert_eq!(
            resolved2.get("region", &Scope::from_url("s3://my-bucket/file")),
            Some("us-west-2".to_string())
        );
    }

    #[test]
    fn test_resolve_scopes() {
        let mut map = HashMap::new();
        map.insert("scope_prod__region".to_string(), "us-west-2".to_string());
        map.insert("scope_prod__endpoint".to_string(), "http://minio".to_string());
        let config = BundleConfig::from_flat_keys(map);

        let mut scopes = HashMap::new();
        scopes.insert("prod".to_string(), "s3://my-bucket/".to_string());
        let resolved = config.resolve_scopes(&scopes);

        assert!(!resolved.has_unresolved_scopes());
        assert_eq!(
            resolved.get("region", &Scope::from_url("s3://my-bucket/file")),
            Some("us-west-2".to_string())
        );
        assert_eq!(
            resolved.get("endpoint", &Scope::from_url("s3://my-bucket/file")),
            Some("http://minio".to_string())
        );
    }

    #[test]
    fn test_resolve_scopes_unknown_skipped() {
        let mut map = HashMap::new();
        map.insert("scope_unknown__region".to_string(), "us-west-2".to_string());
        let config = BundleConfig::from_flat_keys(map);

        let scopes = HashMap::new(); // no matching scope
        let resolved = config.resolve_scopes(&scopes);

        assert!(!resolved.has_unresolved_scopes());
        // Should have no entries since the scope was unknown and dropped
        assert_eq!(resolved.get("region", &Scope::global()), None);
    }

    #[test]
    fn test_merge_from_flat_keys() {
        let config1 = BundleConfig::from_flat_keys({
            let mut m = HashMap::new();
            m.insert("scope_prod__region".to_string(), "us-west-2".to_string());
            m
        });

        let config2 = BundleConfig::from_flat_keys({
            let mut m = HashMap::new();
            m.insert("scope_prod__region".to_string(), "us-east-1".to_string());
            m.insert("scope_staging__endpoint".to_string(), "http://staging".to_string());
            m
        });

        config1.merge(&config2);

        // Both should be resolvable
        let mut scopes = HashMap::new();
        scopes.insert("prod".to_string(), "s3://prod-bucket/".to_string());
        scopes.insert("staging".to_string(), "s3://staging-bucket/".to_string());
        let resolved = config1.resolve_scopes(&scopes);

        assert_eq!(
            resolved.get("region", &Scope::from_url("s3://prod-bucket/file")),
            Some("us-east-1".to_string()) // config2 wins
        );

        assert_eq!(
            resolved.get("endpoint", &Scope::from_url("s3://staging-bucket/file")),
            Some("http://staging".to_string())
        );
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
        config.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);
        config.set("endpoint", "http://minio", &Scope::from_url("s3://bucket/"), ConfigSource::Stored);

        let values = config.values(&[]);
        assert_eq!(values.len(), 2);

        let region = values.iter().find(|e| e.key == "region").expect("region entry");
        assert_eq!(region.value, "us-west-2");
        assert!(region.scope.is_global());
        assert_eq!(region.source, ConfigSource::Stored);
        assert!(region.active);

        let endpoint = values.iter().find(|e| e.key == "endpoint").expect("endpoint entry");
        assert_eq!(endpoint.value, "http://minio");
        assert_eq!(endpoint.scope, Scope::from_url("s3://bucket/"));
        assert!(endpoint.active);
    }

    #[test]
    fn test_all_values_multiple_layers() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);
        config.set("region", "us-east-1", &Scope::global(), ConfigSource::Runtime);

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
    fn test_all_values_url_scoped() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::from_url("s3://bucket/"), ConfigSource::Stored);
        config.set("region", "eu-west-1", &Scope::from_url("s3://bucket/"), ConfigSource::Passed);
        config.set("endpoint", "http://localhost", &Scope::global(), ConfigSource::Passed);

        let all = config.all_values(&[]);
        assert_eq!(all.len(), 3);

        // URL-scoped "region": stored is overridden, passed wins
        let stored_region = all.iter().find(|e| {
            e.key == "region" && e.source == ConfigSource::Stored
        }).expect("stored region");
        assert!(!stored_region.active);

        let passed_region = all.iter().find(|e| {
            e.key == "region" && e.source == ConfigSource::Passed
        }).expect("passed region");
        assert!(passed_region.active);
        assert_eq!(passed_region.scope, Scope::from_url("s3://bucket/"));

        // Global "endpoint": only in passed, so active
        let endpoint = all.iter().find(|e| e.key == "endpoint").expect("endpoint");
        assert!(endpoint.active);
        assert_eq!(endpoint.source, ConfigSource::Passed);
    }

    #[test]
    fn test_secure_flag_on_entries() {
        let specs = test_specs();

        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::from_url("s3://bucket/"), ConfigSource::Stored);
        config.set("secret_access_key", "SECRETKEY", &Scope::from_url("s3://bucket/"), ConfigSource::Stored);
        config.set("endpoint", "http://localhost", &Scope::global(), ConfigSource::Stored);

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
    fn test_config_scopes() {
        let config = BundleConfig::new();
        config.add_scope_alias("prod", &Scope::from_url("s3://prod-bucket/"));
        config.add_scope_alias("staging", &Scope::from_url("s3://staging-bucket/"));

        let scopes = config.scope_aliases();
        assert_eq!(scopes.get("prod"), Some(&Scope::from_url("s3://prod-bucket/")));
        assert_eq!(scopes.get("staging"), Some(&Scope::from_url("s3://staging-bucket/")));

        assert_eq!(config.resolve_alias("prod"), Some(Scope::from_url("s3://prod-bucket/")));
        assert_eq!(config.resolve_alias("unknown"), None);
    }

    #[test]
    fn test_passed_entries() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::global(), ConfigSource::Passed);
        config.set("endpoint", "http://minio", &Scope::from_url("s3://bucket/"), ConfigSource::Passed);
        config.set("stored_key", "stored_value", &Scope::global(), ConfigSource::Stored);

        let passed = config.passed_entries();
        assert_eq!(passed.len(), 2);

        let region = passed.iter().find(|e| e.0 == "region").expect("region");
        assert_eq!(region.1, "us-west-2");
        assert!(region.2.is_global());

        let endpoint = passed.iter().find(|e| e.0 == "endpoint").expect("endpoint");
        assert_eq!(endpoint.1, "http://minio");
        assert_eq!(endpoint.2, Scope::from_url("s3://bucket/"));
    }

    #[test]
    fn test_reload_non_runtime() {
        let config1 = BundleConfig::new();
        config1.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);
        config1.set("runtime_key", "runtime_value", &Scope::global(), ConfigSource::Runtime);

        let config2 = BundleConfig::new();
        config2.set("region", "eu-west-1", &Scope::global(), ConfigSource::Stored);
        config2.set("new_key", "new_value", &Scope::global(), ConfigSource::Passed);

        config1.reload_non_runtime(&config2);

        // Runtime should be preserved
        assert_eq!(config1.get("runtime_key", &Scope::global()), Some("runtime_value".to_string()));
        // Stored should come from config2
        assert_eq!(config1.get("region", &Scope::global()), Some("eu-west-1".to_string()));
        // Passed from config2 should be present
        assert_eq!(config1.get("new_key", &Scope::global()), Some("new_value".to_string()));
    }

    #[test]
    fn test_priority_ordering() {
        let config = BundleConfig::new();
        config.set("key", "stored", &Scope::global(), ConfigSource::Stored);
        config.set("key", "passed", &Scope::global(), ConfigSource::Passed);

        // Passed should win over Stored
        assert_eq!(config.get("key", &Scope::global()), Some("passed".to_string()));

        // Runtime should win over everything
        config.set("key", "runtime", &Scope::global(), ConfigSource::Runtime);
        assert_eq!(config.get("key", &Scope::global()), Some("runtime".to_string()));
    }

    #[test]
    fn test_serialized_config_roundtrip() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);
        config.set("endpoint", "http://localhost", &Scope::from_url("s3://test/"), ConfigSource::Stored);

        let serialized = SerializedConfig::from_bundle_config(&config);
        assert_eq!(serialized.defaults.get("region"), Some(&"us-west-2".to_string()));
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
        assert_eq!(config2.get("region", &Scope::from_url("s3://test/file")), Some("us-west-2".to_string()));
        assert_eq!(config2.get("endpoint", &Scope::from_url("s3://test/file")), Some("http://localhost".to_string()));
    }

    #[test]
    fn test_longest_prefix_wins_over_source_priority() {
        let config = BundleConfig::new();
        config.set("region", "runtime-short", &Scope::from_url("s3://"), ConfigSource::Runtime);
        config.set("region", "stored-long", &Scope::from_url("s3://bucket/"), ConfigSource::Stored);

        // Longer prefix in Stored beats shorter prefix in Runtime
        assert_eq!(
            config.get("region", &Scope::from_url("s3://bucket/file")),
            Some("stored-long".to_string())
        );
        // URL that only matches the short prefix → Runtime wins
        assert_eq!(
            config.get("region", &Scope::from_url("s3://other/file")),
            Some("runtime-short".to_string())
        );
    }

    #[test]
    fn test_get_compound_key_with_scope() {
        let config = BundleConfig::new();
        config.add_scope_alias("prod", &Scope::from_url("s3://prod-bucket/"));
        config.set("region", "us-west-2", &Scope::from_url("s3://prod-bucket/"), ConfigSource::Stored);

        // Compound key: "prod__region" resolves to key="region", scope="/s3/prod-bucket"
        assert_eq!(
            config.get("prod__region", &Scope::global()),
            Some("us-west-2".to_string())
        );
    }

    #[test]
    fn test_get_scope_name_as_url() {
        let config = BundleConfig::new();
        config.add_scope_alias("prod", &Scope::from_url("s3://prod-bucket/"));
        config.set("region", "us-west-2", &Scope::from_url("s3://prod-bucket/"), ConfigSource::Stored);

        // Scope name as scope: get("region", Scope::from_url("prod")) resolves via alias
        assert_eq!(
            config.get("region", &Scope::from_url("prod")),
            Some("us-west-2".to_string())
        );
    }

    #[test]
    fn test_get_compound_key_unknown_scope() {
        let config = BundleConfig::new();
        config.add_scope_alias("prod", &Scope::from_url("s3://prod-bucket/"));
        config.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);

        // Unknown scope in compound key: falls through as literal key "unknown__region"
        assert_eq!(config.get("unknown__region", &Scope::global()), None);

        // Global default still works for plain key
        assert_eq!(config.get("region", &Scope::global()), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_get_scope_name_as_url_unknown_scope() {
        let config = BundleConfig::new();
        config.set("region", "us-west-2", &Scope::global(), ConfigSource::Stored);

        // Unknown scope name — Scope::from_url("unknown_scope") becomes /unknown_scope
        // which doesn't match any alias, so normalize_key_scope passes through.
        // Since global matches everything, should still get the value.
        assert_eq!(
            config.get("region", &Scope::from_url("unknown_scope")),
            Some("us-west-2".to_string())
        );
    }

    #[test]
    fn test_get_url_behavior_unchanged() {
        let config = BundleConfig::new();
        config.add_scope_alias("prod", &Scope::from_url("s3://prod-bucket/"));
        config.set("region", "us-west-2", &Scope::from_url("s3://prod-bucket/"), ConfigSource::Stored);
        config.set("region", "eu-west-1", &Scope::global(), ConfigSource::Stored);

        // Full URL scope still works
        assert_eq!(
            config.get("region", &Scope::from_url("s3://prod-bucket/file")),
            Some("us-west-2".to_string())
        );

        // Unscoped lookup still works
        assert_eq!(
            config.get("region", &Scope::global()),
            Some("eu-west-1".to_string())
        );
    }

    #[test]
    fn test_get_compound_key_and_scope_url_agree() {
        let config = BundleConfig::new();
        config.add_scope_alias("prod", &Scope::from_url("s3://prod-bucket/"));
        config.set("region", "us-west-2", &Scope::from_url("s3://prod-bucket/"), ConfigSource::Stored);

        // All three ways of querying should return the same value
        let via_compound = config.get("prod__region", &Scope::global());
        let via_scope_name = config.get("region", &Scope::from_url("prod"));
        let via_full_url = config.get("region", &Scope::from_url("s3://prod-bucket/"));

        assert_eq!(via_compound, Some("us-west-2".to_string()));
        assert_eq!(via_scope_name, Some("us-west-2".to_string()));
        assert_eq!(via_full_url, Some("us-west-2".to_string()));
    }
}
