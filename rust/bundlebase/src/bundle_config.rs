mod passed;
mod registry;
mod system;
mod scope;
pub use passed::PassedBundleConfig;
pub use system::{SYSTEM_SCOPE, MAX_MEMORY_CFG, CATALOG_NAME_CFG, ALLOW_EXTERNAL_CODE_CFG, is_external_code_allowed};
pub use scope::{Scope, validated_scope, validated_scope_from_url};
use registry::config_registry;

// Re-export config types from common
pub use bundlebase_common::config::{ConfigKey, ConfigProvider, ConfigScope, ConfigSource, default_url_to_name};

use arrow::array::{BooleanArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use std::sync::Arc;

use crate::bundle::command::response::{single_batch_stream, CommandResponse, OutputShape};
use crate::impl_dyn_command_response;
use crate::BundlebaseError;
use datafusion::execution::SendableRecordBatchStream;
use parking_lot::RwLock;
use std::collections::HashMap;

// ConfigSource, ConfigScope, ConfigKey, ConfigProvider are re-exported from bundlebase_common above.

/// A single config entry with source tracking metadata.
#[derive(Debug, Clone)]
pub struct ConfigValueDetails {
    /// Configuration key (e.g., "region", "endpoint")
    pub key: String,
    /// Configuration value
    pub value: String,
    /// Named scope this entry belongs to
    pub scope: Scope,
    /// Which layer this value came from
    pub source: ConfigSource,
    /// True if this entry is the winning value for its key+scope
    pub active: bool,
    /// True if this key holds a secret (password, token, etc.)
    pub secure: bool,
}

// ConfigScope is now defined in bundlebase_common::config

// ConfigKey is now defined in bundlebase_common::config.
// These validation methods remain here since they need the config registry.

/// Check whether a key is secure for a given scope.
pub fn is_key_secure(scope: &Scope, key: &str) -> bool {
    BundleConfig::get_config_key(scope, key)
        .map_or(false, |spec| spec.secure)
}

/// Validate that a config key is recognized for a specific scope.
pub fn validate_key_exists(scope: &Scope, key: &str) -> Result<(), BundlebaseError> {
    if BundleConfig::get_config_key(scope, key).is_some() {
        Ok(())
    } else {
        let valid_keys: Vec<&str> = config_registry().keys()
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

// Re-export macros from common (they use #[macro_export] so they're at the crate root)


/// A single config entry stored internally.
#[derive(Debug, Clone)]
struct ConfigValue {
    /// Named scope (e.g., "s3", "s3/bucket")
    scope: String,
    key: String,
    value: String,
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
    /// The original passed config, preserved for reuse when opening related bundles.
    passed_config: Arc<PassedBundleConfig>,
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
        ConfigScope::new(name)
    }

    /// Returns all known configuration scopes.
    pub fn all_scopes() -> Vec<ConfigScope> {
        config_registry().scopes().to_vec()
    }

    /// Returns all known configuration key specs for validation.
    pub fn all_keys() -> Vec<ConfigKey> {
        config_registry().keys().to_vec()
    }

    /// Look up a registered config key by scope and key name.
    ///
    /// Returns the first matching `ConfigKey` whose scope matches the given scope.
    pub fn get_config_key(scope: &Scope, key: &str) -> Option<ConfigKey> {
        config_registry().keys().iter()
            .find(|spec| spec.key == key && spec.scope.matches(scope))
            .copied()
    }

    /// Create a new configuration, optionally pre-populated with passed entries.
    pub fn new(passed: Option<&PassedBundleConfig>) -> Result<Self, BundlebaseError> {
        let stored = passed.cloned().unwrap_or_default();
        let cfg = Self {
            inner: RwLock::new(ConfigInner::new()),
            passed_config: Arc::new(stored),
        };
        if let Some(passed) = passed {
            for (scope, entries) in passed.iter() {
                for (key, value) in entries {
                    cfg.set(scope, key, value, ConfigSource::Passed)?;
                }
            }
        }
        Ok(cfg)
    }

    /// Returns the original passed config for reuse when opening related bundles.
    pub fn passed_config(&self) -> Arc<PassedBundleConfig> {
        Arc::clone(&self.passed_config)
    }

    /// Set a config value.
    ///
    /// Validates that the key is recognized for the given scope before storing.
    ///
    /// # Arguments
    /// * `scope` - Named scope. Use `Scope::try_from()` to convert raw paths at the call site.
    /// * `key` - Configuration key (e.g., "region", "access_key_id").
    /// * `value` - Configuration value
    /// * `source` - Which config layer this entry belongs to
    pub fn set(&self, scope: &Scope, key: &str, value: &str, source: ConfigSource) -> Result<(), BundlebaseError> {
        validate_key_exists(scope, key)?;

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
        Ok(())
    }

    /// Replace Stored config entries from another config (for `reload_from`).
    ///
    /// Only Stored entries change between reloads (from new SaveConfigOps in the manifest).
    /// Runtime, Passed, and Env entries are unchanged.
    pub fn reload_stored(&self, other: &BundleConfig) {
        let other_inner = other.inner.read();
        let mut self_inner = self.inner.write();

        match other_inner.entries.get(&ConfigSource::Stored) {
            Some(stored) => {
                self_inner.entries.insert(ConfigSource::Stored, stored.clone());
            }
            None => {
                self_inner.entries.remove(&ConfigSource::Stored);
            }
        }

        self_inner.active_cache = None;
        self_inner.winners_cache = None;
    }

    /// Get the winning value for a key, scoped to a parsed Scope.
    ///
    /// Ensures env cache is populated, then finds the longest matching prefix
    /// across all sources. Among entries sharing the longest prefix, the
    /// highest-priority source wins. Only entries whose scope is compatible
    /// with the key's required `ConfigScope` are considered.
    pub fn get(&self, scope: &Scope, key: &ConfigKey) -> Result<Option<String>, BundlebaseError> {
        self.ensure_env_cache()?;

        // Fast path: check active cache with read lock
        {
            let inner = self.inner.read();
            if let Some(cache) = &inner.active_cache {
                if let Some(value) = Self::lookup_active(cache, key, scope) {
                    return Ok(Some(value));
                }
                // Fall back to default if scope is compatible
                if key.scope.matches(scope) {
                    if let Some(value) = key.resolve_default() {
                        return Ok(Some(value));
                    }
                }
                return Ok(None);
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
                    return Ok(Some(value));
                }
                // Fall back to default if scope is compatible
                if key.scope.matches(scope) {
                    if let Some(value) = key.resolve_default() {
                        return Ok(Some(value));
                    }
                }
                Ok(None)
            }
            None => Ok(None), // should not happen after populate_active_cache
        }
    }

    /// Like [`get`], but returns an error if the key is not set.
    ///
    /// `context` is prepended to the error message (e.g. `"Cannot configure Kaggle client: No configuration set for /kaggle:username"`).
    pub fn get_required(&self, scope: &Scope, key: &ConfigKey, context: &str) -> Result<String, BundlebaseError> {
        self.get(scope, key)?.ok_or_else(|| {
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
                let scope = match Scope::from_name(&entry.scope) {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("Skipping invalid scope '{}' in config: {}", entry.scope, e);
                        continue;
                    }
                };
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

        // Sort each key's entries by scope length descending (longest first, global last)
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
    /// Uses the registered config keys to determine which keys are secure
    /// and to append synthetic Default entries.
    pub fn all_values(&self) -> Result<Vec<ConfigValueDetails>, BundlebaseError> {
        let specs = BundleConfig::all_keys();
        self.ensure_env_cache()?;

        // Fast path: check if caches are populated with read lock
        {
            let inner = self.inner.read();
            if let Some(winners) = &inner.winners_cache {
                return Ok(Self::build_all_values(&inner, winners, &specs));
            }
        }

        // Slow path: populate caches with write lock
        let mut inner = self.inner.write();
        if inner.winners_cache.is_none() {
            Self::populate_active_cache(&mut inner);
        }

        let winners = inner.winners_cache.as_ref().expect("just populated");
        Ok(Self::build_all_values(&inner, winners, &specs))
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
                let scope = match Scope::from_name(&entry.scope) {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("Skipping invalid scope '{}' in config: {}", entry.scope, e);
                        continue;
                    }
                };
                let winner_key = (entry.key.clone(), scope.clone());
                let is_active = winners
                    .get(&winner_key)
                    .map_or(false, |(p, _)| *p == priority);
                let is_secure = is_key_secure(&scope, &entry.key);
                result.push(ConfigValueDetails {
                    key: entry.key.clone(),
                    value: entry.value.clone(),
                    scope,
                    source: source.clone(),
                    active: is_active,
                    secure: is_secure,
                });
            }
        }

        // Append synthetic default entries for keys that have defaults
        for spec in specs {
            if let Some(description) = spec.default_description() {
                let scope = match Scope::from_name(spec.scope.name) {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!("Skipping invalid scope '{}' in config spec: {}", spec.scope.name, e);
                        continue;
                    }
                };
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
    pub fn values(&self) -> Result<Vec<ConfigValueDetails>, BundlebaseError> {
        Ok(self.all_values()?
            .into_iter()
            .filter(|e| e.active)
            .collect())
    }

    /// Ensure env vars are loaded into entries[Env]. Reads BB_* env vars on first call.
    /// Env var patterns (suffix after `BB_`):
    /// - `BB_S3_REGION` -> scope `s3`, key = `region` (single `_` separates scope from key)
    /// - `BB_S3__DATA__REGION` -> scope `s3/data`, key = `region` (`__` encodes sub-path separators)
    /// - `BB_KEY` -> skipped (no `_`, no scope)
    /// - `BB__S3__REGION` -> skipped (scope name is empty/starts with `_`)
    fn ensure_env_cache(&self) -> Result<(), BundlebaseError> {
        // Fast path: check with read lock
        {
            let inner = self.inner.read();
            if inner.env_loaded {
                return Ok(());
            }
        }

        // Slow path: double-check and mark loaded under write lock to prevent concurrent re-entry
        {
            let mut inner = self.inner.write();
            if inner.env_loaded {
                return Ok(());
            }
            inner.env_loaded = true;
        }

        // Parse BB_* env vars and store via set() (which validates scope+key)
        for (raw_key, value) in std::env::vars() {
            let Some(suffix) = raw_key.strip_prefix("BB_") else {
                continue;
            };

            let parts: Vec<&str> = suffix.split("__").collect();

            let (scope_str, key) = if parts.len() == 1 {
                // No __ separator: BB_SCOPE_KEY pattern
                let Some(underscore_pos) = suffix.find('_') else {
                    log::warn!(
                        "Ignoring env var '{}': no scope found. \
                         Use BB_SCOPE_KEY format (e.g., BB_S3_REGION).",
                        raw_key
                    );
                    continue;
                };
                (suffix[..underscore_pos].to_lowercase(), suffix[underscore_pos + 1..].to_lowercase())
            } else {
                // __ separators: BB_SCOPE__SUB__...__KEY pattern
                let scope_name = parts[0];
                if scope_name.is_empty() || scope_name.starts_with('_') {
                    log::warn!(
                        "Ignoring env var '{}': scope name must not be empty or start with '_'.",
                        raw_key
                    );
                    continue;
                }
                let key = parts.last().expect("split always returns at least one element").to_lowercase();
                let mut scope_parts: Vec<String> = Vec::with_capacity(parts.len() - 1);
                scope_parts.push(scope_name.to_lowercase());
                for part in &parts[1..parts.len() - 1] {
                    scope_parts.push(part.to_lowercase());
                }
                (scope_parts.join("/"), key)
            };

            let scope = match Scope::new(&scope_str, &BundleConfig::all_scopes()) {
                Ok(s) => s,
                Err(_) => {
                    log::warn!(
                        "Ignoring env var '{}': unknown scope '{}'.",
                        raw_key, scope_str
                    );
                    continue;
                }
            };

            if let Err(e) = self.set(&scope, &key, &value, ConfigSource::Env) {
                log::warn!("Ignoring env var '{}': {}", raw_key, e);
            }
        }

        Ok(())
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
            passed_config: Arc::clone(&self.passed_config),
        }
    }
}

/// Implement ConfigProvider for BundleConfig so IO crates can use it via the trait.
impl ConfigProvider for BundleConfig {
    fn get(&self, scope: &Scope, key: &ConfigKey) -> Result<Option<String>, BundlebaseError> {
        // Delegate to the inherent method
        BundleConfig::get(self, scope, key)
    }
}

/// CommandResponse implementation for displaying config entries as a table.
impl CommandResponse for Vec<ConfigValueDetails> {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("scope", DataType::Utf8, false),
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
            Field::new("source", DataType::Utf8, false),
            Field::new("active", DataType::Boolean, false),
            Field::new("secure", DataType::Boolean, false),
        ]))
    }

    fn output_shape() -> OutputShape {
        OutputShape::Table
    }

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let scopes: Vec<String> = self.iter().map(|e| e.scope.as_str().to_string()).collect();
        let scope_refs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
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
        let sources: Vec<&str> = self.iter().map(|e| e.source.as_str()).collect();
        let actives: Vec<bool> = self.iter().map(|e| e.active).collect();
        let secures: Vec<bool> = self.iter().map(|e| e.secure).collect();

        let batch = RecordBatch::try_new(
            Self::schema(),
            vec![
                Arc::new(StringArray::from(scope_refs)),
                Arc::new(StringArray::from(keys)),
                Arc::new(StringArray::from(value_refs)),
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
    use crate::io::plugin::object_store::{
        S3_SCOPE, GCS_SCOPE,
        S3_REGION_CFG, S3_ENDPOINT_CFG,
        S3_BUCKET_CFG, S3_SKIP_SIGNATURE_CFG,
        S3_IMDSV1_FALLBACK_CFG, S3_CHECKSUM_ALGORITHM_CFG,
        S3_COPY_IF_NOT_EXISTS_CFG, S3_CONDITIONAL_PUT_CFG,
    };

    /// Get a ConfigScope by name from the pre-registered scopes.
    fn get_scope(name: &str) -> ConfigScope {
        BundleConfig::all_scopes()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("Scope '{}' not found in registered scopes", name))
    }

    #[test]
    fn test_set_scoped_default() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "region", "us-west-2", ConfigSource::Stored).unwrap();
        assert_eq!(config.get(&Scope::try_from("s3://bucket/file").unwrap(), &S3_REGION_CFG).unwrap(), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_set_scoped_override() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3://test/").unwrap(), "endpoint", "http://localhost:9000", ConfigSource::Stored).unwrap();

        assert_eq!(
            config.get(&Scope::try_from("s3://test/file").unwrap(), &S3_ENDPOINT_CFG).unwrap(),
            Some("http://localhost:9000".to_string())
        );
    }

    #[test]
    fn test_get_defaults_with_path() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "region", "us-west-2", ConfigSource::Stored).unwrap();

        assert_eq!(config.get(&Scope::try_from("s3://my-bucket/path/to/file").unwrap(), &S3_REGION_CFG).unwrap(), Some("us-west-2".to_string()));
    }

    #[test]
    fn test_get_with_scoped_override() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "region", "us-west-2", ConfigSource::Stored).unwrap();
        config.set(&Scope::try_from("s3://special-bucket/").unwrap(), "region", "us-east-1", ConfigSource::Stored).unwrap();

        assert_eq!(config.get(&Scope::try_from("s3://my-bucket/file").unwrap(), &S3_REGION_CFG).unwrap(), Some("us-west-2".to_string()));
        assert_eq!(config.get(&Scope::try_from("s3://special-bucket/file").unwrap(), &S3_REGION_CFG).unwrap(), Some("us-east-1".to_string()));
    }

    #[test]
    fn test_longest_prefix_matching() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3://bucket/").unwrap(), "endpoint", "default", ConfigSource::Stored).unwrap();
        config.set(&Scope::try_from("s3://bucket/subfolder/").unwrap(), "endpoint", "specific", ConfigSource::Stored).unwrap();

        // Should match the longer prefix
        assert_eq!(config.get(&Scope::try_from("s3://bucket/subfolder/file").unwrap(), &S3_ENDPOINT_CFG).unwrap(), Some("specific".to_string()));

        // Should match the shorter prefix
        assert_eq!(config.get(&Scope::try_from("s3://bucket/otherpath/file").unwrap(), &S3_ENDPOINT_CFG).unwrap(), Some("default".to_string()));
    }

    // from_env tests use unique env var names to avoid conflicts between parallel tests
    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_from_env_single_segment_skipped() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_TESTREGION1", "us-west-2");

        let config = BundleConfig::new(None).unwrap();
        // Force env cache load
        config.ensure_env_cache().unwrap();

        // BB_TESTREGION1 is a single-segment env var (no __), so it should
        // be skipped entirely — not stored at any scope
        assert_eq!(
            config.get(&Scope::try_from("s3://bucket/").unwrap(), &S3_IMDSV1_FALLBACK_CFG).unwrap(),
            None
        );
        std::env::remove_var("BB_TESTREGION1");
    }

    #[test]
    fn test_from_env_scoped() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3_CHECKSUM_ALGORITHM", "sha256");

        let config = BundleConfig::new(None).unwrap();
        config.ensure_env_cache().unwrap();
        // BB_S3_CHECKSUM_ALGORITHM -> scope "s3", key "checksum_algorithm"
        assert_eq!(
            config.get(&Scope::try_from("s3").unwrap(), &S3_CHECKSUM_ALGORITHM_CFG).unwrap(),
            Some("sha256".to_string())
        );
        // Should also match via prefix matching on child paths
        assert_eq!(
            config.get(&Scope::try_from("s3/bucket").unwrap(), &S3_CHECKSUM_ALGORITHM_CFG).unwrap(),
            Some("sha256".to_string())
        );
        std::env::remove_var("BB_S3_CHECKSUM_ALGORITHM");
    }

    #[test]
    fn test_from_env_multi_segment_scope() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3__MY_BUCKET__CONDITIONAL_PUT", "etag");

        let config = BundleConfig::new(None).unwrap();
        config.ensure_env_cache().unwrap();
        // BB_S3__MY_BUCKET__CONDITIONAL_PUT -> scope "s3/my_bucket", key "conditional_put"
        assert_eq!(
            config.get(&Scope::try_from("s3/my_bucket").unwrap(), &S3_CONDITIONAL_PUT_CFG).unwrap(),
            Some("etag".to_string())
        );
        std::env::remove_var("BB_S3__MY_BUCKET__CONDITIONAL_PUT");
    }

    #[test]
    fn test_from_env_empty() {
        // from_env with no BB_ vars should not crash
        let config = BundleConfig::new(None).unwrap();
        config.ensure_env_cache().unwrap();
        let _ = config;
    }

    #[test]
    fn test_from_env_scope_with_underscore_key() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3_COPY_IF_NOT_EXISTS", "multipart");

        let config = BundleConfig::new(None).unwrap();
        config.ensure_env_cache().unwrap();
        // BB_S3_COPY_IF_NOT_EXISTS -> scope "s3", key "copy_if_not_exists"
        assert_eq!(
            config.get(&Scope::try_from("s3").unwrap(), &S3_COPY_IF_NOT_EXISTS_CFG).unwrap(),
            Some("multipart".to_string())
        );
        std::env::remove_var("BB_S3_COPY_IF_NOT_EXISTS");
    }

    #[test]
    fn test_from_env_leading_double_underscore_rejected() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB__S3__CHECKSUM_ALGORITHM", "should-be-skipped");

        let config = BundleConfig::new(None).unwrap();
        config.ensure_env_cache().unwrap();
        // BB__S3__CHECKSUM_ALGORITHM -> first part is "_S3" (starts with _), should be skipped
        assert_eq!(
            config.get(&Scope::try_from("s3").unwrap(), &S3_CHECKSUM_ALGORITHM_CFG).unwrap(),
            None
        );
        std::env::remove_var("BB__S3__CHECKSUM_ALGORITHM");
    }

    #[test]
    fn test_from_env_deep_sub_path() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3__A__B__CONDITIONAL_PUT", "deep-value");

        let config = BundleConfig::new(None).unwrap();
        config.ensure_env_cache().unwrap();
        // BB_S3__A__B__CONDITIONAL_PUT -> scope "s3/a/b", key "conditional_put"
        assert_eq!(
            config.get(&Scope::try_from("s3/a/b").unwrap(), &S3_CONDITIONAL_PUT_CFG).unwrap(),
            Some("deep-value".to_string())
        );
        std::env::remove_var("BB_S3__A__B__CONDITIONAL_PUT");
    }

    #[test]
    fn test_from_env_two_segment_with_double_underscore() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3__CHECKSUM_ALGORITHM", "double-us");

        let config = BundleConfig::new(None).unwrap();
        config.ensure_env_cache().unwrap();
        // BB_S3__CHECKSUM_ALGORITHM -> split on __ -> ["S3", "CHECKSUM_ALGORITHM"]
        // scope "s3", key "checksum_algorithm"
        assert_eq!(
            config.get(&Scope::try_from("s3").unwrap(), &S3_CHECKSUM_ALGORITHM_CFG).unwrap(),
            Some("double-us".to_string())
        );
        std::env::remove_var("BB_S3__CHECKSUM_ALGORITHM");
    }

    #[test]
    fn test_from_env_case_insensitive() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3_CHECKSUM_ALGORITHM", "case-test");

        let config = BundleConfig::new(None).unwrap();
        config.ensure_env_cache().unwrap();
        // BB_S3_CHECKSUM_ALGORITHM -> scope "s3", key "checksum_algorithm" (all lowercase)
        assert_eq!(
            config.get(&Scope::try_from("s3").unwrap(), &S3_CHECKSUM_ALGORITHM_CFG).unwrap(),
            Some("case-test".to_string())
        );
        std::env::remove_var("BB_S3_CHECKSUM_ALGORITHM");
    }

    #[test]
    fn test_values_empty() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        // No explicitly set values; only synthetic Default entries from registered keys
        assert!(config.values().unwrap().iter().all(|e| e.source == ConfigSource::Default));
        assert!(config.all_values().unwrap().iter().all(|e| e.source == ConfigSource::Default));
    }

    #[test]
    fn test_values_single_layer() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "region", "us-west-2", ConfigSource::Stored).unwrap();
        config.set(&Scope::try_from("s3://bucket/").unwrap(), "endpoint", "http://minio", ConfigSource::Stored).unwrap();

        let values: Vec<_> = config.values().unwrap().into_iter()
            .filter(|e| e.source != ConfigSource::Default)
            .collect();
        assert_eq!(values.len(), 2);

        let region = values.iter().find(|e| e.key == "region").expect("region entry");
        assert_eq!(region.value, "us-west-2");
        assert_eq!(region.scope, Scope::try_from("s3").unwrap());
        assert_eq!(region.source, ConfigSource::Stored);
        assert!(region.active);

        let endpoint = values.iter().find(|e| e.key == "endpoint").expect("endpoint entry");
        assert_eq!(endpoint.value, "http://minio");
        assert_eq!(endpoint.scope, Scope::try_from("s3://bucket/").unwrap());
        assert!(endpoint.active);
    }

    #[test]
    fn test_all_values_multiple_layers() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "region", "us-west-2", ConfigSource::Stored).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "region", "us-east-1", ConfigSource::Runtime).unwrap();

        let all: Vec<_> = config.all_values().unwrap().into_iter()
            .filter(|e| e.source != ConfigSource::Default)
            .collect();
        assert_eq!(all.len(), 2);

        let stored_entry = all.iter().find(|e| e.source == ConfigSource::Stored).expect("stored entry");
        assert_eq!(stored_entry.value, "us-west-2");
        assert!(!stored_entry.active, "stored should be overridden");

        let runtime_entry = all.iter().find(|e| e.source == ConfigSource::Runtime).expect("runtime entry");
        assert_eq!(runtime_entry.value, "us-east-1");
        assert!(runtime_entry.active, "runtime should win");

        // values() should only return the winner (excluding defaults)
        let active: Vec<_> = config.values().unwrap().into_iter()
            .filter(|e| e.source != ConfigSource::Default)
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].value, "us-east-1");
        assert_eq!(active[0].source, ConfigSource::Runtime);
    }

    #[test]
    fn test_all_values_scoped() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3://bucket/").unwrap(), "region", "us-west-2", ConfigSource::Stored).unwrap();
        config.set(&Scope::try_from("s3://bucket/").unwrap(), "region", "eu-west-1", ConfigSource::Passed).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "endpoint", "http://localhost", ConfigSource::Passed).unwrap();

        let all: Vec<_> = config.all_values().unwrap().into_iter()
            .filter(|e| e.source != ConfigSource::Default)
            .collect();
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
        assert_eq!(passed_region.scope, Scope::try_from("s3://bucket/").unwrap());

        // Scoped "endpoint": only in passed, so active
        let endpoint = all.iter().find(|e| e.key == "endpoint").expect("endpoint");
        assert!(endpoint.active);
        assert_eq!(endpoint.source, ConfigSource::Passed);
    }

    #[test]
    fn test_secure_flag_on_entries() {
        let _lock = ENV_MUTEX.lock();

        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3://bucket/").unwrap(), "region", "us-west-2", ConfigSource::Stored).unwrap();
        config.set(&Scope::try_from("s3://bucket/").unwrap(), "secret_access_key", "SECRETKEY", ConfigSource::Stored).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "endpoint", "http://localhost", ConfigSource::Stored).unwrap();

        let all = config.all_values().unwrap();

        let region = all.iter().find(|e| e.key == "region").expect("region");
        assert!(!region.secure);

        let secret = all.iter().find(|e| e.key == "secret_access_key").expect("secret");
        assert!(secret.secure);

        let endpoint = all.iter().find(|e| e.key == "endpoint").expect("endpoint");
        assert!(!endpoint.secure);
    }

    #[test]
    fn test_is_key_secure() {
        let s3 = Scope::try_from("s3").unwrap();
        let gcs = Scope::try_from("gs").unwrap();
        let azure = Scope::try_from("azure").unwrap();

        // Secure keys (scoped)
        assert!(is_key_secure(&s3, "secret_access_key"));
        assert!(is_key_secure(&s3, "session_token"));
        assert!(is_key_secure(&azure, "access_key"));
        assert!(is_key_secure(&gcs, "service_account_key"));
        assert!(is_key_secure(&azure, "client_secret"));

        // Non-secure keys
        assert!(!is_key_secure(&s3, "region"));
        assert!(!is_key_secure(&azure, "account"));
        assert!(!is_key_secure(&s3, "bucket"));

        // Secure key but wrong scope — not secure
        assert!(!is_key_secure(&gcs, "secret_access_key"));

        // Unknown key — not secure
        assert!(!is_key_secure(&s3, "nonexistent_key"));
    }

    #[test]
    fn test_reload_stored() {
        let _lock = ENV_MUTEX.lock();
        let config1 = BundleConfig::new(None).unwrap();
        config1.set(&Scope::try_from("s3").unwrap(), "region", "us-west-2", ConfigSource::Stored).unwrap();
        config1.set(&Scope::try_from("s3").unwrap(), "skip_signature", "runtime_value", ConfigSource::Runtime).unwrap();
        config1.set(&Scope::try_from("s3").unwrap(), "bucket", "original_passed", ConfigSource::Passed).unwrap();

        let config2 = BundleConfig::new(None).unwrap();
        config2.set(&Scope::try_from("s3").unwrap(), "region", "eu-west-1", ConfigSource::Stored).unwrap();
        config2.set(&Scope::try_from("s3").unwrap(), "bucket", "config2_passed", ConfigSource::Passed).unwrap();

        config1.reload_stored(&config2);

        // Runtime should be preserved
        assert_eq!(config1.get(&Scope::try_from("s3://bucket/").unwrap(), &S3_SKIP_SIGNATURE_CFG).unwrap(), Some("runtime_value".to_string()));
        // Stored should come from config2
        assert_eq!(config1.get(&Scope::try_from("s3://bucket/").unwrap(), &S3_REGION_CFG).unwrap(), Some("eu-west-1".to_string()));
        // Passed from config1 should be preserved (not replaced by config2's Passed)
        assert_eq!(config1.get(&Scope::try_from("s3://bucket/").unwrap(), &S3_BUCKET_CFG).unwrap(), Some("original_passed".to_string()));
    }

    #[test]
    fn test_priority_ordering() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "region", "stored", ConfigSource::Stored).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "region", "passed", ConfigSource::Passed).unwrap();

        // Passed should win over Stored
        assert_eq!(config.get(&Scope::try_from("s3://bucket/").unwrap(), &S3_REGION_CFG).unwrap(), Some("passed".to_string()));

        // Runtime should win over everything
        config.set(&Scope::try_from("s3").unwrap(), "region", "runtime", ConfigSource::Runtime).unwrap();
        assert_eq!(config.get(&Scope::try_from("s3://bucket/").unwrap(), &S3_REGION_CFG).unwrap(), Some("runtime".to_string()));
    }

    #[test]
    fn test_longest_prefix_wins_over_source_priority() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3://").unwrap(), "region", "runtime-short", ConfigSource::Runtime).unwrap();
        config.set(&Scope::try_from("s3://bucket/").unwrap(), "region", "stored-long", ConfigSource::Stored).unwrap();

        // Longer prefix in Stored beats shorter prefix in Runtime
        assert_eq!(
            config.get(&Scope::try_from("s3://bucket/file").unwrap(), &S3_REGION_CFG).unwrap(),
            Some("stored-long".to_string())
        );
        // Path that only matches the short prefix → Runtime wins
        assert_eq!(
            config.get(&Scope::try_from("s3://other/file").unwrap(), &S3_REGION_CFG).unwrap(),
            Some("runtime-short".to_string())
        );
    }

    // ── ConfigScope tests ────────────────────────────────────────────

    #[test]
    fn test_config_scope_matches_exact() {
        let scope = get_scope("s3");
        assert!(scope.matches(&Scope::try_from("s3").unwrap()));
    }

    #[test]
    fn test_config_scope_matches_child() {
        let scope = get_scope("s3");
        assert!(scope.matches(&Scope::try_from("s3/bucket").unwrap()));
        assert!(scope.matches(&Scope::try_from("s3/bucket/path").unwrap()));
    }

    #[test]
    fn test_config_scope_rejects_different_prefix() {
        let scope = get_scope("s3");
        assert!(!scope.matches(&Scope::try_from("kaggle").unwrap()));
    }

    #[test]
    fn test_config_scope_rejects_different_provider() {
        let scope = get_scope("s3");
        assert!(!scope.matches(&Scope::try_from("gs/bucket").unwrap()));
        assert!(!scope.matches(&Scope::try_from("azure/container").unwrap()));
    }

    #[test]
    fn test_config_scope_rejects_partial_prefix() {
        // "s3x" is not a registered scope, so validated_scope rejects it
        assert!(validated_scope("s3x").is_err());
    }

    #[test]
    fn test_all_scopes() {
        let scopes = BundleConfig::all_scopes();
        let names: Vec<&str> = scopes.iter().map(|s| s.name).collect();
        assert!(names.contains(&"system"), "all_scopes should include system scope");
        assert!(names.contains(&"s3"));
        assert!(names.contains(&"gs"));
        assert!(names.contains(&"azure"));
        assert!(names.contains(&"ftp"));
        assert!(names.contains(&"sftp"));
        #[cfg(feature = "connector-kaggle")]
        assert!(names.contains(&"kaggle"));
    }

    #[test]
    fn test_all_scopes_system_is_first() {
        let scopes = BundleConfig::all_scopes();
        assert_eq!(scopes[0].name, "system", "system scope should be first in all_scopes");
    }

    #[test]
    fn test_system_scope_matches() {
        use crate::bundle_config::SYSTEM_SCOPE;
        assert!(SYSTEM_SCOPE.matches(&Scope::try_from("system").unwrap()));
        assert!(SYSTEM_SCOPE.matches(&Scope::try_from("system/sub").unwrap()));
        assert!(!SYSTEM_SCOPE.matches(&Scope::try_from("s3").unwrap()));
        assert!(!SYSTEM_SCOPE.matches(&Scope::try_from("s3/bucket").unwrap()));
    }

    #[test]
    fn test_system_scope_url_to_name() {
        use crate::bundle_config::SYSTEM_SCOPE;
        assert_eq!(SYSTEM_SCOPE.url_to_name("system://settings"), Some("system/settings".to_string()));
        assert_eq!(SYSTEM_SCOPE.url_to_name("system://"), Some("system".to_string()));
        assert_eq!(SYSTEM_SCOPE.url_to_name("s3://bucket"), None);
        assert_eq!(SYSTEM_SCOPE.url_to_name("anything"), None);
    }

    #[test]
    fn test_system_scope_parse() {
        // Name-based parsing
        assert_eq!(Scope::try_from("system").unwrap().as_str(), "system");
        assert_eq!(Scope::try_from("system/sub").unwrap().as_str(), "system/sub");
        // URL-based parsing
        assert_eq!(Scope::try_from("system://settings").unwrap().as_str(), "system/settings");
    }

    #[test]
    fn test_validate_key_exists() {
        // "region" is in S3 scope
        assert!(validate_key_exists(&Scope::try_from("s3").unwrap(), "region").is_ok());
        assert!(validate_key_exists(&Scope::try_from("s3/bucket").unwrap(), "region").is_ok());
        // "region" is NOT in GCS scope
        assert!(validate_key_exists(&Scope::try_from("gs").unwrap(), "region").is_err());
        // "account" is in Azure scope
        assert!(validate_key_exists(&Scope::try_from("azure").unwrap(), "account").is_ok());
        assert!(validate_key_exists(&Scope::try_from("s3").unwrap(), "account").is_err());
    }

    // ── URL-to-name conversion tests ─────────────────────────────────

    #[test]
    fn test_default_url_to_name_matching_scheme() {
        let scope = get_scope("s3");
        let result = default_url_to_name(&scope, "s3://bucket/path");
        assert_eq!(result, Some("s3/bucket/path".to_string()));
    }

    #[test]
    fn test_default_url_to_name_non_matching_scheme() {
        let scope = get_scope("s3");
        let result = default_url_to_name(&scope, "gs://bucket/path");
        assert_eq!(result, None);
    }

    #[test]
    fn test_default_url_to_name_non_url() {
        let scope = get_scope("s3");
        // Non-URL input should not match (name handling is in Scope::try_from)
        assert_eq!(default_url_to_name(&scope, "s3/bucket"), None);
        assert_eq!(default_url_to_name(&scope, "s3"), None);
        assert_eq!(default_url_to_name(&scope, "gs/bucket"), None);
    }

    #[test]
    fn test_config_scope_url_to_name_delegates() {
        let scope = get_scope("s3");
        assert_eq!(scope.url_to_name("s3://bucket"), Some("s3/bucket".to_string()));
        assert_eq!(scope.url_to_name("gs://bucket"), None);
    }

    #[cfg(feature = "connector-kaggle")]
    #[test]
    fn test_config_scope_with_custom_url_to_name() {
        // Test that Kaggle scope (which has a custom url_to_name) works correctly
        let scope = get_scope("kaggle");
        assert_eq!(scope.url_to_name("kaggle://user/dataset"), Some("kaggle/dataset".to_string()));
        // Non-URL strings return None
        assert_eq!(scope.url_to_name("not a url"), None);
    }

    #[test]
    fn test_scope_parse_s3_url() {
        assert_eq!(Scope::try_from("s3://bucket/path").unwrap(), Scope::try_from("s3/bucket/path").unwrap());
    }

    #[test]
    fn test_scope_parse_name() {
        // Name-based parsing through Scope::try_from
        assert_eq!(Scope::try_from("s3").unwrap().as_str(), "s3");
        assert_eq!(Scope::try_from("s3/bucket").unwrap().as_str(), "s3/bucket");
    }

    #[test]
    fn test_scope_parse_unknown_errors() {
        // validated_scope checks against the registry, so unknown scopes are rejected
        let result = validated_scope("not-a-valid-scope");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown scope"), "Expected 'Unknown scope' in: {}", err);
    }

    // ── ConfigKey default value tests ─────────────────────────────────

    // Test keys with defaults - use S3_SCOPE as the real scope
    const TEST_KEY_WITH_DEFAULT: ConfigKey = S3_SCOPE
        .define("allow_http")
        .with_default("https://default.example.com");
    const TEST_KEY_NO_DEFAULT: ConfigKey = S3_SCOPE.define("test_no_default");

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
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        assert_eq!(
            config.get(&Scope::try_from("s3").unwrap(), &TEST_KEY_WITH_DEFAULT).unwrap(),
            Some("https://default.example.com".to_string())
        );
        // Also matches child scopes
        assert_eq!(
            config.get(&Scope::try_from("s3://bucket").unwrap(), &TEST_KEY_WITH_DEFAULT).unwrap(),
            Some("https://default.example.com".to_string())
        );
    }

    #[test]
    fn test_get_returns_none_for_incompatible_scope_even_with_default() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        // A different scope should not return the default
        assert_eq!(
            config.get(&Scope::try_from("gs").unwrap(), &TEST_KEY_WITH_DEFAULT).unwrap(),
            None
        );
    }

    #[test]
    fn test_get_returns_explicit_value_over_default() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("s3").unwrap(), "allow_http", "https://custom.example.com", ConfigSource::Stored).unwrap();
        assert_eq!(
            config.get(&Scope::try_from("s3").unwrap(), &TEST_KEY_WITH_DEFAULT).unwrap(),
            Some("https://custom.example.com".to_string())
        );
    }

    // Default-value behavior in all_values() is covered by get() tests above.
    // Synthetic Default entries in all_values() use the registered config keys.

    // ── ConfigKey default_fn tests ────────────────────────────────────

    fn test_default_fn_value() -> Option<String> {
        Some("dynamic_value".to_string())
    }

    fn test_default_fn_none() -> Option<String> {
        None
    }

    // Test keys with default functions - use GCS_SCOPE to avoid conflicts
    const TEST_KEY_WITH_DEFAULT_FN: ConfigKey = GCS_SCOPE
        .define("service_account_path")
        .with_default_fn("test source", test_default_fn_value);
    const TEST_KEY_WITH_DEFAULT_FN_NONE: ConfigKey = GCS_SCOPE
        .define("test_none_key")
        .with_default_fn("test source", test_default_fn_none);

    #[test]
    fn test_with_default_fn_const_context() {
        assert!(TEST_KEY_WITH_DEFAULT_FN.default_fn.is_some());
        let (desc, _) = TEST_KEY_WITH_DEFAULT_FN.default_fn.expect("just checked");
        assert_eq!(desc, "test source");
    }

    #[test]
    fn test_get_returns_default_fn_value() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        assert_eq!(
            config.get(&Scope::try_from("gs").unwrap(), &TEST_KEY_WITH_DEFAULT_FN).unwrap(),
            Some("dynamic_value".to_string())
        );
        assert_eq!(
            config.get(&Scope::try_from("gs://bucket").unwrap(), &TEST_KEY_WITH_DEFAULT_FN).unwrap(),
            Some("dynamic_value".to_string())
        );
    }

    #[test]
    fn test_get_returns_none_when_default_fn_returns_none() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        assert_eq!(
            config.get(&Scope::try_from("gs").unwrap(), &TEST_KEY_WITH_DEFAULT_FN_NONE).unwrap(),
            None
        );
    }

    #[test]
    fn test_get_returns_none_for_incompatible_scope_with_default_fn() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        assert_eq!(
            config.get(&Scope::try_from("s3").unwrap(), &TEST_KEY_WITH_DEFAULT_FN).unwrap(),
            None
        );
    }

    #[test]
    fn test_default_fn_takes_priority_over_default_value() {
        let _lock = ENV_MUTEX.lock();
        // When both default_value and default_fn are set, default_fn wins
        const KEY_BOTH: ConfigKey = GCS_SCOPE
            .define("test_both_key")
            .with_default("static_value")
            .with_default_fn("test source", test_default_fn_value);

        let config = BundleConfig::new(None).unwrap();
        assert_eq!(
            config.get(&Scope::try_from("gs").unwrap(), &KEY_BOTH).unwrap(),
            Some("dynamic_value".to_string())
        );
    }

    #[test]
    fn test_default_value_used_when_no_default_fn() {
        let _lock = ENV_MUTEX.lock();
        const KEY_STATIC_ONLY: ConfigKey = GCS_SCOPE
            .define("test_static_only")
            .with_default("static_value");

        let config = BundleConfig::new(None).unwrap();
        assert_eq!(
            config.get(&Scope::try_from("gs").unwrap(), &KEY_STATIC_ONLY).unwrap(),
            Some("static_value".to_string())
        );
    }

    #[test]
    fn test_get_returns_explicit_value_over_default_fn() {
        let _lock = ENV_MUTEX.lock();
        let config = BundleConfig::new(None).unwrap();
        config.set(&Scope::try_from("gs").unwrap(), "service_account_path", "explicit", ConfigSource::Stored).unwrap();
        assert_eq!(
            config.get(&Scope::try_from("gs").unwrap(), &TEST_KEY_WITH_DEFAULT_FN).unwrap(),
            Some("explicit".to_string())
        );
    }

    // Default-fn behavior in all_values() is covered by get() tests above.
    // Synthetic Default entries in all_values() use the registered config keys.

}
