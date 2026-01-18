use crate::BundlebaseError;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use url::Url;

// Valid config keys for different cloud providers
// Based on object_store crate's ConfigKey enums
lazy_static! {
    static ref VALID_S3_KEYS: HashSet<&'static str> = {
        vec![
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
        ]
        .into_iter()
        .collect()
    };
    static ref VALID_GCS_KEYS: HashSet<&'static str> = {
        vec![
            "service_account_key",
            "service_account_path",
            "bucket",
            "application_credentials",
        ]
        .into_iter()
        .collect()
    };
    static ref VALID_AZURE_KEYS: HashSet<&'static str> = {
        vec![
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
        ]
        .into_iter()
        .collect()
    };
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
    ///
    /// # Errors
    /// Returns error if:
    /// - URL keys don't have object/map values
    /// - Config values are not strings
    /// - Config keys are invalid for the specified cloud provider
    pub fn from_map(map: HashMap<String, Value>) -> Result<Self, BundlebaseError> {
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
                    Self::validate_key(&key, inner_key)?;
                    config.set(inner_key, inner_str, Some(&key));
                }
            } else {
                // Default setting
                let value_str = value.as_str().ok_or_else(|| {
                    BundlebaseError::from("Config value must be string".to_string())
                })?;
                Self::validate_key("", &key)?;
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

    /// Validate a config key against known object_store keys for the specified cloud provider
    ///
    /// # Arguments
    /// * `url_prefix` - URL prefix to determine cloud provider (empty string for defaults)
    /// * `key` - Configuration key to validate
    ///
    /// # Errors
    /// Returns error if the key is not valid for the specified cloud provider
    fn validate_key(url_prefix: &str, key: &str) -> Result<(), BundlebaseError> {
        // Determine cloud provider from URL prefix
        let valid_keys = if url_prefix.starts_with("s3://") || url_prefix.is_empty() {
            &*VALID_S3_KEYS // Default to S3 keys for validation
        } else if url_prefix.starts_with("gs://") {
            &*VALID_GCS_KEYS
        } else if url_prefix.starts_with("azure://") || url_prefix.starts_with("az://") {
            &*VALID_AZURE_KEYS
        } else {
            // Unknown scheme, allow any keys
            return Ok(());
        };

        if !valid_keys.contains(key) {
            let keys_list: Vec<&str> = valid_keys.iter().copied().collect();
            return Err(format!(
                "Invalid config key '{}' for {}. Valid keys: {:?}",
                key,
                if url_prefix.is_empty() {
                    "defaults"
                } else {
                    url_prefix
                },
                keys_list
            )
            .into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_validate_key_valid_s3() {
        assert!(BundleConfig::validate_key("", "region").is_ok());
        assert!(BundleConfig::validate_key("s3://bucket/", "access_key_id").is_ok());
    }

    #[test]
    fn test_validate_key_invalid() {
        let result = BundleConfig::validate_key("", "invalid_key");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid config key 'invalid_key'"));
    }

    #[test]
    fn test_validate_key_gcs() {
        assert!(BundleConfig::validate_key("gs://bucket/", "service_account_key").is_ok());
        let result = BundleConfig::validate_key("gs://bucket/", "region");
        assert!(result.is_err());
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
}
