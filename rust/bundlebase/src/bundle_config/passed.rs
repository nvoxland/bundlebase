use super::Scope;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Lightweight data-transfer config passed to `create()` / `open()`.
///
/// Unlike [`BundleConfig`](super::BundleConfig) (which carries RwLock, caches,
/// and multi-source priority tracking), `PassedBundleConfig` is a plain,
/// cloneable value type that holds only the key-value pairs the caller wants
/// to inject as `ConfigSource::Passed` entries.
///
/// Flat-key patterns (`scope__key`) should be parsed at the boundary
/// (Python bindings, env loading) before constructing this type.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PassedBundleConfig {
    /// Global config entries: key -> value
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub defaults: HashMap<String, String>,

    /// Scope-specific config: scope -> (key -> value)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scoped: HashMap<Scope, HashMap<String, String>>,
}

impl PassedBundleConfig {
    /// Create an empty config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a config value, routing to `defaults` or `scoped` based on the scope.
    pub fn set(&mut self, key: &str, value: &str, scope: &Scope) {
        if scope.is_global() {
            self.defaults.insert(key.to_string(), value.to_string());
        } else {
            self.scoped
                .entry(scope.clone())
                .or_default()
                .insert(key.to_string(), value.to_string());
        }
    }

    /// Returns true if there are no entries at all.
    pub fn is_empty(&self) -> bool {
        self.defaults.is_empty() && self.scoped.is_empty()
    }

    /// Merge another `PassedBundleConfig` into this one. `other` wins on conflict.
    pub fn merge(&mut self, other: &PassedBundleConfig) {
        for (key, value) in &other.defaults {
            self.defaults.insert(key.clone(), value.clone());
        }
        for (scope, entries) in &other.scoped {
            let target = self.scoped.entry(scope.clone()).or_default();
            for (key, value) in entries {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_empty() {
        let cfg = PassedBundleConfig::new();
        assert!(cfg.is_empty());
        assert!(cfg.defaults.is_empty());
        assert!(cfg.scoped.is_empty());
    }

    #[test]
    fn test_set_global() {
        let mut cfg = PassedBundleConfig::new();
        cfg.set("region", "us-west-2", &Scope::global());
        assert!(!cfg.is_empty());
        assert_eq!(cfg.defaults.get("region"), Some(&"us-west-2".to_string()));
        assert!(cfg.scoped.is_empty());
    }

    #[test]
    fn test_set_scoped() {
        let mut cfg = PassedBundleConfig::new();
        let scope = Scope::normalize("s3://bucket/");
        cfg.set("endpoint", "http://localhost:9000", &scope);
        assert!(!cfg.is_empty());
        assert!(cfg.defaults.is_empty());
        assert_eq!(
            cfg.scoped.get(&scope).and_then(|m| m.get("endpoint")),
            Some(&"http://localhost:9000".to_string())
        );
    }

    #[test]
    fn test_set_overwrites() {
        let mut cfg = PassedBundleConfig::new();
        cfg.set("region", "us-west-2", &Scope::global());
        cfg.set("region", "us-east-1", &Scope::global());
        assert_eq!(cfg.defaults.get("region"), Some(&"us-east-1".to_string()));
    }

    #[test]
    fn test_merge_defaults() {
        let mut cfg1 = PassedBundleConfig::new();
        cfg1.set("region", "us-west-2", &Scope::global());
        cfg1.set("endpoint", "old", &Scope::global());

        let mut cfg2 = PassedBundleConfig::new();
        cfg2.set("region", "us-east-1", &Scope::global());
        cfg2.set("access_key_id", "KEY123", &Scope::global());

        cfg1.merge(&cfg2);

        assert_eq!(cfg1.defaults.get("region"), Some(&"us-east-1".to_string()));
        assert_eq!(cfg1.defaults.get("endpoint"), Some(&"old".to_string()));
        assert_eq!(
            cfg1.defaults.get("access_key_id"),
            Some(&"KEY123".to_string())
        );
    }

    #[test]
    fn test_merge_scoped() {
        let scope = Scope::normalize("s3://bucket/");

        let mut cfg1 = PassedBundleConfig::new();
        cfg1.set("region", "us-west-2", &scope);

        let mut cfg2 = PassedBundleConfig::new();
        cfg2.set("region", "us-east-1", &scope);
        cfg2.set("endpoint", "http://new", &scope);

        cfg1.merge(&cfg2);

        let scoped = cfg1.scoped.get(&scope).expect("scope should exist");
        assert_eq!(scoped.get("region"), Some(&"us-east-1".to_string()));
        assert_eq!(scoped.get("endpoint"), Some(&"http://new".to_string()));
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut cfg = PassedBundleConfig::new();
        cfg.set("region", "us-west-2", &Scope::global());
        cfg.set(
            "endpoint",
            "http://localhost",
            &Scope::normalize("s3://test/"),
        );

        let json = serde_json::to_string(&cfg).expect("serialize");
        let deserialized: PassedBundleConfig =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, deserialized);
    }

    #[test]
    fn test_serde_empty_skips_fields() {
        let cfg = PassedBundleConfig::new();
        let json = serde_json::to_string(&cfg).expect("serialize");
        assert_eq!(json, "{}");
    }
}
