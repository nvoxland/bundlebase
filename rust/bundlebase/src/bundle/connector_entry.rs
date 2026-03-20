//! Connector entry system for named, platform-aware connector entrypoints.
//!
//! A `ConnectorEntry` is created via `IMPORT CONNECTOR acme.weather`
//! and represents a single connector entrypoint binding for a name+platform pair.
//! `resolve_connector` picks the best entry for the current platform at runtime.

use crate::platform::Platform;
use crate::data::ObjectId;
use crate::namespaced_name::NamespacedName;
use crate::BundlebaseError;

use crate::udf::UdfRuntime;

/// A single connector entry binding a name+platform to runtime+entrypoint.
///
/// Multiple entries can exist for the same connector name (different platforms
/// or temporary vs persisted). Resolution picks the best match at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorEntry {
    pub id: ObjectId,
    pub name: NamespacedName,
    pub from: UdfRuntime,
    pub platform: Platform,
    pub temporary: bool,
}

/// Resolve the best connector entry for the current platform.
///
/// Tries temporary entries first (reverse order, last wins), then persisted entries.
/// Returns the first entry whose platform matches the current system.
pub fn resolve_connector(entries: &[ConnectorEntry], name: &str) -> Result<ConnectorEntry, BundlebaseError> {
    let matching: Vec<&ConnectorEntry> = entries.iter().filter(|e| e.name == name).collect();

    if matching.is_empty() {
        return Err(format!("Connector '{}' is not defined", name).into());
    }

    // Try temporary entries first (reverse order, last wins)
    for entry in matching.iter().rev() {
        if entry.temporary && entry.platform.matches_current() {
            return Ok((*entry).clone());
        }
    }

    // Then persisted entries (reverse order, last wins)
    for entry in matching.iter().rev() {
        if !entry.temporary && entry.platform.matches_current() {
            return Ok((*entry).clone());
        }
    }

    let platforms: Vec<String> = matching.iter().map(|e| e.platform.to_string()).collect();
    Err(format!(
        "No connector entrypoint matches current platform '{}' for connector '{}'. Available platforms: {}",
        Platform::current(),
        name,
        platforms.join(", ")
    )
    .into())
}

/// Parse and validate a dotted connector name.
///
/// Enforces single-level dotted namespace: exactly one dot, both parts alphanumeric.
///
/// # Examples
/// - `"acme.weather"` → `Ok(("acme", "weather"))`
/// - `"acme.datasources.weather"` → error (multi-level)
/// - `"weather"` → error (no dot)
pub fn parse_connector_name(name: &str) -> Result<NamespacedName, BundlebaseError> {
    crate::namespaced_name::NamespacedName::parse(name, "Connector")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_last_set_wins() {
        let entries = vec![
            ConnectorEntry {
                id: ObjectId::generate(),
                name: NamespacedName::new("test", "source"),
                from: UdfRuntime::parse_from("ffi::first").unwrap(),
                platform: Platform::any(),
                temporary: false,
            },
            ConnectorEntry {
                id: ObjectId::generate(),
                name: NamespacedName::new("test", "source"),
                from: UdfRuntime::parse_from("ffi::second").unwrap(),
                platform: Platform::any(),
                temporary: false,
            },
        ];

        let resolved = resolve_connector(&entries, "test.source").expect("should resolve");
        assert_eq!(resolved.from.to_entrypoint_string(), "second");
    }

    #[test]
    fn test_resolve_no_match() {
        let entries = vec![ConnectorEntry {
            id: ObjectId::generate(),
            name: NamespacedName::new("test", "source"),
            from: UdfRuntime::parse_from("ffi::test").unwrap(),
            platform: "nonexistent/arch".parse().unwrap(),
            temporary: false,
        }];

        let result = resolve_connector(&entries, "test.source");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No connector entrypoint matches"));
    }

    #[test]
    fn test_resolve_not_defined() {
        let entries: Vec<ConnectorEntry> = vec![];
        let result = resolve_connector(&entries, "test.source");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("is not defined"));
    }

    #[test]
    fn test_resolve_temporary_overrides_persisted() {
        let entries = vec![
            ConnectorEntry {
                id: ObjectId::generate(),
                name: NamespacedName::new("test", "source"),
                from: UdfRuntime::parse_from("ffi::persisted").unwrap(),
                platform: Platform::any(),
                temporary: false,
            },
            ConnectorEntry {
                id: ObjectId::generate(),
                name: NamespacedName::new("test", "source"),
                from: UdfRuntime::parse_from("python::temp:source").unwrap(),
                platform: Platform::any(),
                temporary: true,
            },
        ];

        let resolved = resolve_connector(&entries, "test.source").expect("should resolve");
        assert_eq!(resolved.from.to_entrypoint_string(), "temp:source");
        assert!(resolved.temporary);
    }

    #[test]
    fn test_parse_connector_name_valid() {
        let nn = parse_connector_name("acme.weather").unwrap();
        assert_eq!(nn.namespace, "acme");
        assert_eq!(nn.name, "weather");
    }

    #[test]
    fn test_parse_connector_name_no_dot() {
        let result = parse_connector_name("weather");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be in format 'namespace.name'"));
    }

    #[test]
    fn test_parse_connector_name_multi_level_rejected() {
        let result = parse_connector_name("acme.datasources.weather");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Multi-level namespaces"));
    }

    #[test]
    fn test_parse_connector_name_rejects_non_alphanumeric() {
        assert!(parse_connector_name("acme.bad-name").is_err());
    }
}
