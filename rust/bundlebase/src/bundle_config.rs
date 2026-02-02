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
    /// * `value` - Configuration value
    /// * `scope` - Normalized scope, or global for default.
    ///             Use `Scope::from_url()` to convert raw URLs at the call site.
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

    /// Get the winning value for a key, scoped to a normalized Scope.
    ///
    /// Ensures env cache is populated, then finds the longest matching prefix
    /// across all sources. Among entries sharing the longest prefix, the
    /// highest-priority source wins. Pass `Scope::global()` for an unscoped lookup.
    pub fn get(&self, key: &str, scope: &Scope) -> Option<String> {
        self.ensure_env_cache();

        // Fast path: check active cache with read lock
        {
            let inner = self.inner.read();
            if let Some(cache) = &inner.active_cache {
                return Self::lookup_active(cache, key, scope);
            }
        }

        // Slow path: populate cache with write lock
        let mut inner = self.inner.write();

        // Double-check after upgrading to write lock
        if inner.active_cache.is_none() {
            Self::populate_active_cache(&mut inner);
        }

        match &inner.active_cache {
            Some(cache) => Self::lookup_active(cache, key, scope),
            None => None, // should not happen after populate_active_cache
        }
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

    /// Check if a key looks like a URL (contains "://")
    fn is_url_key(key: &str) -> bool {
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
    fn test_from_env_scoped() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3__TESTREGION2", "us-west-2");

        let config = BundleConfig::new();
        config.ensure_env_cache();
        // BB_S3__TESTREGION2 -> scope "/s3", key "testregion2"
        assert_eq!(
            config.get("testregion2", &Scope::new("/s3")),
            Some("us-west-2".to_string())
        );
        // Should also match via prefix matching on child paths
        assert_eq!(
            config.get("testregion2", &Scope::new("/s3/bucket")),
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
            config.get("testkey3", &Scope::new("/s3/my_bucket")),
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

}
