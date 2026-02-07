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
    /// Scope-specific config: scope -> (key -> value)
    /// todo: just wrap a map?
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scoped: HashMap<Scope, HashMap<String, String>>,
}

impl PassedBundleConfig {
    /// Create an empty config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a config value for a named scope.
    pub fn set(&mut self, scope: &Scope, key: &str, value: &str) {
        self.scoped
            .entry(scope.clone())
            .or_default()
            .insert(key.to_string(), value.to_string());
    }

    /// Returns true if there are no entries at all.
    pub fn is_empty(&self) -> bool {
        self.scoped.is_empty()
    }

    /// Merge another `PassedBundleConfig` into this one. `other` wins on conflict.
    /// todo: don't need?
    pub fn merge(&mut self, other: &PassedBundleConfig) {
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
        assert!(cfg.scoped.is_empty());
    }

    #[test]
    fn test_set_scoped() {
        let mut cfg = PassedBundleConfig::new();
        let scope = Scope::try_from("s3://bucket").unwrap();
        cfg.set(&scope, "endpoint", "http://localhost:9000");
        assert!(!cfg.is_empty());
        assert_eq!(
            cfg.scoped.get(&scope).and_then(|m| m.get("endpoint")),
            Some(&"http://localhost:9000".to_string())
        );
    }

    #[test]
    fn test_set_overwrites() {
        let mut cfg = PassedBundleConfig::new();
        let scope = Scope::try_from("s3").unwrap();
        cfg.set(&scope, "region", "us-west-2");
        cfg.set(&scope, "region", "us-east-1");
        assert_eq!(
            cfg.scoped.get(&scope).and_then(|m| m.get("region")),
            Some(&"us-east-1".to_string())
        );
    }

    #[test]
    fn test_merge_scoped() {
        let scope = Scope::try_from("s3://bucket").unwrap();

        let mut cfg1 = PassedBundleConfig::new();
        cfg1.set(&scope, "region", "us-west-2");
        cfg1.set(&scope, "endpoint", "old");

        let mut cfg2 = PassedBundleConfig::new();
        cfg2.set(&scope, "region", "us-east-1");
        cfg2.set(&scope, "access_key_id", "KEY123");

        cfg1.merge(&cfg2);

        let scoped = cfg1.scoped.get(&scope).expect("scope should exist");
        assert_eq!(scoped.get("region"), Some(&"us-east-1".to_string()));
        assert_eq!(scoped.get("endpoint"), Some(&"old".to_string()));
        assert_eq!(scoped.get("access_key_id"), Some(&"KEY123".to_string()));
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut cfg = PassedBundleConfig::new();
        cfg.set(&Scope::try_from("s3").unwrap(), "region", "us-west-2");
        cfg.set(
            &Scope::try_from("s3://test").unwrap(),
            "endpoint",
            "http://localhost",
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
