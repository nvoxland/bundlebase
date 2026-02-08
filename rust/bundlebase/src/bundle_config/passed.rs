use super::Scope;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

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
#[serde(transparent)]
pub struct PassedBundleConfig(HashMap<Scope, HashMap<String, String>>);

impl Deref for PassedBundleConfig {
    type Target = HashMap<Scope, HashMap<String, String>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for PassedBundleConfig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl PassedBundleConfig {
    /// Create an empty config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a config value for a named scope.
    pub fn set(&mut self, scope: &Scope, key: &str, value: &str) {
        self.0
            .entry(scope.clone())
            .or_default()
            .insert(key.to_string(), value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_empty() {
        let cfg = PassedBundleConfig::new();
        assert!(cfg.is_empty());
    }

    #[test]
    fn test_set_scoped() {
        let mut cfg = PassedBundleConfig::new();
        let scope = Scope::try_from("s3://bucket").unwrap();
        cfg.set(&scope, "endpoint", "http://localhost:9000");
        assert!(!cfg.is_empty());
        assert_eq!(
            cfg.get(&scope).and_then(|m| m.get("endpoint")),
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
            cfg.get(&scope).and_then(|m| m.get("region")),
            Some(&"us-east-1".to_string())
        );
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
