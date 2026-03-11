//! Validated namespace.name pairs for functions and connectors.
//!
//! A `NamespacedName` stores the namespace and name parts separately but
//! serializes/deserializes as the dotted string form (e.g., `"acme.double_val"`)
//! for backwards compatibility with existing YAML/JSON.

use crate::BundlebaseError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// A validated namespace.name pair for functions and connectors.
///
/// Stores the namespace and name parts separately but serializes/deserializes
/// as the dotted string form (e.g., `"acme.double_val"`) for backwards compatibility.
#[derive(Debug, Clone, Eq, Hash)]
pub struct NamespacedName {
    pub namespace: String,
    pub name: String,
}

impl NamespacedName {
    /// Create a new NamespacedName from already-split parts.
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    /// Parse a dotted string into a NamespacedName, validating the format.
    pub fn parse(dotted: &str, entity_type: &str) -> Result<Self, BundlebaseError> {
        let (namespace, name) = parse_dotted_name(dotted, entity_type)?;
        Ok(Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
        })
    }

    /// Returns the full dotted name as an owned String.
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.namespace, self.name)
    }
}

impl fmt::Display for NamespacedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.namespace, self.name)
    }
}

impl FromStr for NamespacedName {
    type Err = BundlebaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NamespacedName::parse(s, "Name")
    }
}

impl PartialEq for NamespacedName {
    fn eq(&self, other: &Self) -> bool {
        self.namespace == other.namespace && self.name == other.name
    }
}

impl PartialEq<str> for NamespacedName {
    fn eq(&self, other: &str) -> bool {
        self.full_name() == other
    }
}

impl PartialEq<&str> for NamespacedName {
    fn eq(&self, other: &&str) -> bool {
        self.full_name() == *other
    }
}

impl Serialize for NamespacedName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for NamespacedName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Shared validation for dotted names (functions and connectors).
///
/// Enforces single-level dotted namespace: exactly one dot, both parts must
/// start with a letter or underscore and contain only alphanumeric characters
/// and underscores.
pub fn parse_dotted_name<'a>(name: &'a str, entity_type: &str) -> Result<(&'a str, &'a str), BundlebaseError> {
    let parts: Vec<&str> = name.split('.').collect();

    if parts.len() < 2 {
        return Err(format!(
            "{} name must be in format 'namespace.name' (e.g., 'acme.my_func'), got '{}'",
            entity_type, name
        )
        .into());
    }

    if parts.len() > 2 {
        return Err(format!(
            "{} name must be in format 'namespace.name' (e.g., 'acme.my_func'), got '{}'. Multi-level namespaces are not supported.",
            entity_type, name
        )
        .into());
    }

    let namespace = parts[0];
    let short_name = parts[1];

    // Validate both parts are valid identifiers
    fn is_valid_identifier(s: &str) -> bool {
        if s.is_empty() {
            return false;
        }
        let mut chars = s.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    if !is_valid_identifier(namespace) {
        return Err(format!(
            "{} namespace '{}' must start with a letter or underscore and contain only alphanumeric characters and underscores",
            entity_type, namespace
        )
        .into());
    }

    if !is_valid_identifier(short_name) {
        return Err(format!(
            "{} name part '{}' must start with a letter or underscore and contain only alphanumeric characters and underscores",
            entity_type, short_name
        )
        .into());
    }

    Ok((namespace, short_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namespaced_name_display() {
        let nn = NamespacedName::new("acme", "double_val");
        assert_eq!(nn.to_string(), "acme.double_val");
    }

    #[test]
    fn test_namespaced_name_full_name() {
        let nn = NamespacedName::new("acme", "double_val");
        assert_eq!(nn.full_name(), "acme.double_val");
    }

    #[test]
    fn test_namespaced_name_from_str() {
        let nn: NamespacedName = "acme.double_val".parse().unwrap();
        assert_eq!(nn.namespace, "acme");
        assert_eq!(nn.name, "double_val");
    }

    #[test]
    fn test_namespaced_name_from_str_invalid() {
        assert!("no_dot".parse::<NamespacedName>().is_err());
        assert!("a.b.c".parse::<NamespacedName>().is_err());
    }

    #[test]
    fn test_namespaced_name_eq_str() {
        let nn = NamespacedName::new("acme", "double_val");
        assert!(nn == "acme.double_val");
        assert!(nn != "other.name");
    }

    #[test]
    fn test_namespaced_name_eq_str_ref() {
        let nn = NamespacedName::new("acme", "double_val");
        let s: &str = "acme.double_val";
        assert!(nn == s);
    }

    #[test]
    fn test_namespaced_name_serde_roundtrip() {
        let nn = NamespacedName::new("acme", "double_val");
        let yaml = serde_yaml_ng::to_string(&nn).unwrap();
        assert!(yaml.contains("acme.double_val"));
        let deser: NamespacedName = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(deser, nn);
    }

    #[test]
    fn test_namespaced_name_parse_method() {
        let nn = NamespacedName::parse("acme.weather", "Connector").unwrap();
        assert_eq!(nn.namespace, "acme");
        assert_eq!(nn.name, "weather");

        let err = NamespacedName::parse("no_dot", "Connector");
        assert!(err.is_err());
    }

    #[test]
    fn test_parse_dotted_name_rejects_multi_level() {
        let result = parse_dotted_name("a.b.c", "Connector");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Multi-level namespaces are not supported"));
    }

    #[test]
    fn test_parse_dotted_name_accepts_single_level() {
        let (ns, name) = parse_dotted_name("acme.weather", "Connector").unwrap();
        assert_eq!(ns, "acme");
        assert_eq!(name, "weather");
    }

    #[test]
    fn test_parse_dotted_name_rejects_non_alphanumeric() {
        assert!(parse_dotted_name("acme.bad-name", "Connector").is_err());
        assert!(parse_dotted_name("bad!.name", "Connector").is_err());
    }
}
