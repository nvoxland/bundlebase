use crate::BundlebaseError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use url::Url;

/// Defines valid configuration keys for a URL scheme prefix.
/// Each service/provider defines its own spec as a constant.
#[derive(Debug, Clone)]
pub struct ConfigKeySpec {
    /// URL scheme prefix (e.g., "s3://", "kaggle://")
    pub scheme_prefix: &'static str,
    /// Valid configuration keys for this service
    pub valid_keys: &'static [&'static str],
}

/// Configuration for container storage and cloud providers
///
/// # Format
/// The configuration uses a nested structure where:
/// - Top-level keys (non-URL) are default settings applied to all URLs
/// - URL keys (containing "://") contain nested configuration for specific URL prefixes
///
/// # Example
/// ```rust
/// use bundlebase::bundle_config::BundleConfig;
///
/// let mut config = BundleConfig::new();
/// config.set("region", "us-west-2", None);  // Default for all S3
/// config.set("endpoint", "http://localhost:9000", Some("s3://test-bucket/"));  // Override
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BundleConfig {
    /// Default settings for all cloud storage URLs (non-URL keys)
    #[serde(default)]
    defaults: HashMap<String, String>,

    /// URL-specific overrides (key is URL prefix like "s3://bucket/")
    #[serde(default)]
    url_overrides: HashMap<String, HashMap<String, String>>,
}

impl BundleConfig {
    /// Create a new empty configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Create BundleConfig from a nested HashMap (e.g., from Python dict)
    ///
    /// Top-level non-URL keys are defaults, URL keys contain nested config.
    /// URL-scoped keys are validated against the provided specs.
    /// Global/default keys are accepted without validation.
    ///
    /// # Errors
    /// Returns error if:
    /// - URL keys don't have object/map values
    /// - Config values are not strings
    /// - URL-scoped config keys are invalid for the matching scheme
    pub fn from_map(
        map: HashMap<String, Value>,
        specs: &[ConfigKeySpec],
    ) -> Result<Self, BundlebaseError> {
        let mut config = Self::new();

        for (key, value) in map {
            if Self::is_url_key(&key) {
                // URL-specific override
                let url_config = value.as_object().ok_or_else(|| {
                    BundlebaseError::from(format!("URL key '{}' must have object value", key))
                })?;

                for (inner_key, inner_value) in url_config {
                    let inner_str = inner_value.as_str().ok_or_else(|| {
                        BundlebaseError::from("Config value must be string".to_string())
                    })?;
                    Self::validate_url_scoped_key(&key, inner_key, specs)?;
                    config.set(inner_key, inner_str, Some(&key));
                }
            } else {
                // Default setting — no validation, any key is accepted
                let value_str = value.as_str().ok_or_else(|| {
                    BundlebaseError::from("Config value must be string".to_string())
                })?;
                config.set(&key, value_str, None);
            }
        }

        Ok(config)
    }

    /// Set a config value
    ///
    /// # Arguments
    /// * `key` - Configuration key (e.g., "region", "access_key_id")
    /// * `value` - Configuration value
    /// * `url_prefix` - Optional URL prefix for URL-specific config (e.g., "s3://bucket/")
    ///                  If None, this is a default setting
    pub fn set(&mut self, key: &str, value: &str, url_prefix: Option<&str>) {
        match url_prefix {
            Some(prefix) => {
                self.url_overrides
                    .entry(prefix.to_string())
                    .or_default()
                    .insert(key.to_string(), value.to_string());
            }
            None => {
                self.defaults.insert(key.to_string(), value.to_string());
            }
        }
    }

    /// Merge another config into this one, with the other config taking priority
    ///
    /// # Arguments
    /// * `other` - The config to merge (takes priority over self)
    ///
    /// # Returns
    /// A new BundleConfig with merged values
    pub fn merge(&self, other: &BundleConfig) -> BundleConfig {
        let mut merged = BundleConfig::new();

        // Start with self's defaults
        merged.defaults = self.defaults.clone();
        // Override with other's defaults
        merged.defaults.extend(other.defaults.clone());

        // Merge URL overrides - start with self's
        merged.url_overrides = self.url_overrides.clone();
        // Add/override with other's URL overrides
        for (url_prefix, override_map) in &other.url_overrides {
            merged
                .url_overrides
                .entry(url_prefix.clone())
                .or_default()
                .extend(override_map.clone());
        }

        merged
    }

    /// Get config for a specific URL using longest prefix matching
    ///
    /// Returns a HashMap with config values, starting with defaults and merging
    /// URL-specific overrides if a matching prefix is found.
    ///
    /// # Arguments
    /// * `url` - The URL to get configuration for
    ///
    /// # Returns
    /// HashMap of config key-value pairs applicable to this URL
    pub(crate) fn get_config_for_url(&self, url: &Url) -> HashMap<String, String> {
        // 1. Start with defaults
        let mut config = self.defaults.clone();

        // 2. Find longest matching URL prefix
        let url_str = url.to_string();
        let mut best_match: Option<(&String, &HashMap<String, String>)> = None;

        for (prefix, override_config) in &self.url_overrides {
            if url_str.starts_with(prefix) {
                let is_better = match best_match {
                    None => true,
                    Some((prev_prefix, _)) => prefix.len() > prev_prefix.len(),
                };
                if is_better {
                    best_match = Some((prefix, override_config));
                }
            }
        }

        // 3. Merge URL-specific overrides (override_config wins)
        if let Some((_, override_config)) = best_match {
            config.extend(override_config.clone());
        }

        config
    }

    /// Check if a key looks like a URL (contains "://")
    fn is_url_key(key: &str) -> bool {
        key.contains("://")
    }

    /// Validate a URL-scoped config key against registered specs.
    /// Only called for keys under a URL prefix — global defaults are not validated.
    fn validate_url_scoped_key(
        url_prefix: &str,
        key: &str,
        specs: &[ConfigKeySpec],
    ) -> Result<(), BundlebaseError> {
        for spec in specs {
            if url_prefix.starts_with(spec.scheme_prefix) {
                if !spec.valid_keys.contains(&key) {
                    return Err(format!(
                        "Invalid config key '{}' for {}. Valid keys: {:?}",
                        key, url_prefix, spec.valid_keys
                    )
                    .into());
                }
                return Ok(());
            }
        }
        // Unknown scheme with no registered spec — allow any keys
        Ok(())
    }

    /// Load configuration from environment variables.
    ///
    /// Patterns:
    /// - `BB_{KEY}` -> global default (key lowercased)
    /// - `BB_{SCHEME}__{KEY}` -> scheme-level scope (e.g., `BB_S3__REGION` -> `s3://`)
    /// - `BB_SCOPE_{NAME}__{KEY}` -> named scope (resolved via `scopes` map)
    ///
    /// Named scopes require a matching entry in the `scopes` map.
    /// Unknown named scopes are silently skipped (they may not apply to this bundle).
    pub fn from_env(scopes: &HashMap<String, String>) -> Self {
        let mut config = Self::new();

        for (raw_key, value) in std::env::vars() {
            let Some(suffix) = raw_key.strip_prefix("BB_") else {
                continue;
            };

            if let Some(scope_rest) = suffix.strip_prefix("SCOPE_") {
                // BB_SCOPE_{NAME}__{KEY}
                if let Some((scope_name, key)) = scope_rest.split_once("__") {
                    if let Some(url_prefix) = scopes.get(&scope_name.to_lowercase()) {
                        config.set(&key.to_lowercase(), &value, Some(url_prefix));
                    }
                }
            } else if let Some((scheme, key)) = suffix.split_once("__") {
                // BB_{SCHEME}__{KEY}
                let url_prefix = format!("{}://", scheme.to_lowercase());
                config.set(&key.to_lowercase(), &value, Some(&url_prefix));
            } else {
                // BB_{KEY}
                config.set(&suffix.to_lowercase(), &value, None);
            }
        }

        config
    }

    /// Get config for a named service (e.g., "kaggle").
    ///
    /// Looks up keys stored under the `{service}://` URL prefix,
    /// merged with defaults. Services use this to read their config
    /// and apply their own fallback logic for missing keys.
    pub fn get_service_config(&self, service: &str) -> HashMap<String, String> {
        match Url::parse(&format!("{}://config", service)) {
            Ok(url) => self.get_config_for_url(&url),
            Err(_) => self.defaults.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test specs for validation tests.
    fn test_specs() -> Vec<ConfigKeySpec> {
        vec![
            ConfigKeySpec {
                scheme_prefix: "s3://",
                valid_keys: &[
                    "region",
                    "access_key_id",
                    "secret_access_key",
                    "session_token",
                    "endpoint",
                    "bucket",
                    "allow_http",
                    "skip_signature",
                    "virtual_hosted_style_request",
                    "token",
                    "imdsv1_fallback",
                    "metadata_endpoint",
                    "container_credentials_relative_uri",
                    "unsigned_payload",
                    "checksum_algorithm",
                    "copy_if_not_exists",
                    "conditional_put",
                ],
            },
            ConfigKeySpec {
                scheme_prefix: "gs://",
                valid_keys: &[
                    "service_account_key",
                    "service_account_path",
                    "bucket",
                    "application_credentials",
                ],
            },
            ConfigKeySpec {
                scheme_prefix: "azure://",
                valid_keys: &[
                    "account",
                    "access_key",
                    "container",
                    "sas_token",
                    "bearer_token",
                    "client_id",
                    "client_secret",
                    "tenant_id",
                    "authority_host",
                    "use_emulator",
                ],
            },
        ]
    }

    #[test]
    fn test_new_config() {
        let config = BundleConfig::new();
        assert_eq!(config.defaults.len(), 0);
        assert_eq!(config.url_overrides.len(), 0);
    }

    #[test]
    fn test_set_default() {
        let mut config = BundleConfig::new();
        config.set("region", "us-west-2", None);
        assert_eq!(
            config.defaults.get("region"),
            Some(&"us-west-2".to_string())
        );
    }

    #[test]
    fn test_set_url_override() {
        let mut config = BundleConfig::new();
        config.set("endpoint", "http://localhost:9000", Some("s3://test/"));

        let url_config = config.url_overrides.get("s3://test/").unwrap();
        assert_eq!(
            url_config.get("endpoint"),
            Some(&"http://localhost:9000".to_string())
        );
    }

    #[test]
    fn test_get_config_for_url_defaults_only() {
        let mut config = BundleConfig::new();
        config.set("region", "us-west-2", None);

        let url = Url::parse("s3://my-bucket/path/to/file").unwrap();
        let result = config.get_config_for_url(&url);

        assert_eq!(result.get("region"), Some(&"us-west-2".to_string()));
    }

    #[test]
    fn test_get_config_for_url_with_override() {
        let mut config = BundleConfig::new();
        config.set("region", "us-west-2", None);
        config.set("region", "us-east-1", Some("s3://special-bucket/"));

        let url1 = Url::parse("s3://my-bucket/file").unwrap();
        let result1 = config.get_config_for_url(&url1);
        assert_eq!(result1.get("region"), Some(&"us-west-2".to_string()));

        let url2 = Url::parse("s3://special-bucket/file").unwrap();
        let result2 = config.get_config_for_url(&url2);
        assert_eq!(result2.get("region"), Some(&"us-east-1".to_string()));
    }

    #[test]
    fn test_longest_prefix_matching() {
        let mut config = BundleConfig::new();
        config.set("endpoint", "default", Some("s3://bucket/"));
        config.set("endpoint", "specific", Some("s3://bucket/subfolder/"));

        // Should match the longer prefix
        let url = Url::parse("s3://bucket/subfolder/file").unwrap();
        let result = config.get_config_for_url(&url);
        assert_eq!(result.get("endpoint"), Some(&"specific".to_string()));

        // Should match the shorter prefix
        let url2 = Url::parse("s3://bucket/otherpath/file").unwrap();
        let result2 = config.get_config_for_url(&url2);
        assert_eq!(result2.get("endpoint"), Some(&"default".to_string()));
    }

    #[test]
    fn test_is_url_key() {
        assert!(BundleConfig::is_url_key("s3://bucket/"));
        assert!(BundleConfig::is_url_key("gs://bucket/"));
        assert!(!BundleConfig::is_url_key("region"));
        assert!(!BundleConfig::is_url_key("access_key_id"));
    }

    #[test]
    fn test_validate_url_scoped_key_valid_s3() {
        let specs = test_specs();
        assert!(
            BundleConfig::validate_url_scoped_key("s3://bucket/", "access_key_id", &specs).is_ok()
        );
        assert!(
            BundleConfig::validate_url_scoped_key("s3://bucket/", "region", &specs).is_ok()
        );
    }

    #[test]
    fn test_validate_url_scoped_key_invalid() {
        let specs = test_specs();
        let result =
            BundleConfig::validate_url_scoped_key("s3://bucket/", "invalid_key", &specs);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid config key 'invalid_key'"));
    }

    #[test]
    fn test_validate_url_scoped_key_gcs() {
        let specs = test_specs();
        assert!(BundleConfig::validate_url_scoped_key(
            "gs://bucket/",
            "service_account_key",
            &specs
        )
        .is_ok());
        let result =
            BundleConfig::validate_url_scoped_key("gs://bucket/", "region", &specs);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_url_scoped_key_unknown_scheme() {
        let specs = test_specs();
        // Unknown scheme with no spec allows any keys
        assert!(BundleConfig::validate_url_scoped_key(
            "custom://host/",
            "anything_goes",
            &specs
        )
        .is_ok());
    }

    #[test]
    fn test_from_map_defaults_not_validated() {
        // Global defaults should accept any key, not just S3 keys
        let specs = test_specs();
        let mut map = HashMap::new();
        map.insert(
            "custom_setting".to_string(),
            Value::String("value".to_string()),
        );
        let result = BundleConfig::from_map(map, &specs);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap().defaults.get("custom_setting"),
            Some(&"value".to_string())
        );
    }

    #[test]
    fn test_merge() {
        let mut config1 = BundleConfig::new();
        config1.set("region", "us-west-2", None);
        config1.set("endpoint", "old", Some("s3://bucket/"));

        let mut config2 = BundleConfig::new();
        config2.set("region", "us-east-1", None); // Override
        config2.set("access_key_id", "KEY123", None); // New

        let merged = config1.merge(&config2);

        assert_eq!(
            merged.defaults.get("region"),
            Some(&"us-east-1".to_string())
        );
        assert_eq!(
            merged.defaults.get("access_key_id"),
            Some(&"KEY123".to_string())
        );
        assert_eq!(
            merged
                .url_overrides
                .get("s3://bucket/")
                .unwrap()
                .get("endpoint"),
            Some(&"old".to_string())
        );
    }

    #[test]
    fn test_serialization() {
        let mut config = BundleConfig::new();
        config.set("region", "us-west-2", None);
        config.set("endpoint", "http://localhost", Some("s3://test/"));

        let serialized = serde_yaml_ng::to_string(&config).unwrap();
        let deserialized: BundleConfig = serde_yaml_ng::from_str(&serialized).unwrap();

        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_get_service_config() {
        let mut config = BundleConfig::new();
        config.set("global_key", "global_value", None);
        config.set("username", "kaggle_user", Some("kaggle://"));
        config.set("key", "kaggle_key", Some("kaggle://"));

        let kaggle_config = config.get_service_config("kaggle");
        assert_eq!(
            kaggle_config.get("username"),
            Some(&"kaggle_user".to_string())
        );
        assert_eq!(
            kaggle_config.get("key"),
            Some(&"kaggle_key".to_string())
        );
        // Defaults are merged in
        assert_eq!(
            kaggle_config.get("global_key"),
            Some(&"global_value".to_string())
        );
    }

    #[test]
    fn test_get_service_config_empty() {
        let config = BundleConfig::new();
        let kaggle_config = config.get_service_config("kaggle");
        assert!(kaggle_config.is_empty());
    }

    // from_env tests use unique env var names to avoid conflicts between parallel tests
    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_from_env_global_default() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_TESTREGION1", "us-west-2");
        let scopes = HashMap::new();
        let config = BundleConfig::from_env(&scopes);
        assert_eq!(
            config.defaults.get("testregion1"),
            Some(&"us-west-2".to_string())
        );
        std::env::remove_var("BB_TESTREGION1");
    }

    #[test]
    fn test_from_env_scheme_scoped() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_S3__TESTREGION2", "us-west-2");
        let scopes = HashMap::new();
        let config = BundleConfig::from_env(&scopes);
        let url = Url::parse("s3://bucket/file").unwrap();
        let url_config = config.get_config_for_url(&url);
        assert_eq!(
            url_config.get("testregion2"),
            Some(&"us-west-2".to_string())
        );
        std::env::remove_var("BB_S3__TESTREGION2");
    }

    #[test]
    fn test_from_env_named_scope() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_SCOPE_TESTPROD__TESTENDPOINT1", "http://minio");
        let mut scopes = HashMap::new();
        scopes.insert("testprod".to_string(), "s3://bucket/".to_string());
        let config = BundleConfig::from_env(&scopes);
        let url = Url::parse("s3://bucket/file").unwrap();
        let url_config = config.get_config_for_url(&url);
        assert_eq!(
            url_config.get("testendpoint1"),
            Some(&"http://minio".to_string())
        );
        std::env::remove_var("BB_SCOPE_TESTPROD__TESTENDPOINT1");
    }

    #[test]
    fn test_from_env_named_scope_case_insensitive() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_SCOPE_TestProd2__TESTKEY1", "value");
        let mut scopes = HashMap::new();
        scopes.insert("testprod2".to_string(), "s3://bucket2/".to_string());
        let config = BundleConfig::from_env(&scopes);
        let url = Url::parse("s3://bucket2/file").unwrap();
        let url_config = config.get_config_for_url(&url);
        assert_eq!(url_config.get("testkey1"), Some(&"value".to_string()));
        std::env::remove_var("BB_SCOPE_TestProd2__TESTKEY1");
    }

    #[test]
    fn test_from_env_unknown_named_scope_skipped() {
        let _lock = ENV_MUTEX.lock();
        std::env::set_var("BB_SCOPE_UNKNOWN99__TESTKEY2", "value");
        let scopes = HashMap::new(); // no matching scope
        let config = BundleConfig::from_env(&scopes);
        // Should be empty since scope is unknown
        assert!(config.defaults.is_empty());
        assert!(config.url_overrides.is_empty());
        std::env::remove_var("BB_SCOPE_UNKNOWN99__TESTKEY2");
    }

    #[test]
    fn test_from_env_empty() {
        // from_env with no BB_ vars should return empty config
        // (other BB_ vars from other tests don't matter since we check specific keys)
        let scopes = HashMap::new();
        let config = BundleConfig::from_env(&scopes);
        // Just verify no crash - other BB_ vars from parallel tests may exist
        let _ = config;
    }
}
