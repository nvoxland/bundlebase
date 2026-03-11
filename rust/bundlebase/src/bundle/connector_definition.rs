//! Connector entry system for named, platform-aware connector logic.
//!
//! A `ConnectorEntry` is created via `IMPORT CONNECTOR acme.weather`
//! and represents a single connector logic binding for a name+platform pair.
//! `resolve_connector` picks the best entry for the current platform at runtime.

use crate::namespaced_name::NamespacedName;
use crate::BundlebaseError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A Docker-style platform identifier in `os/arch` format.
///
/// Supports `*` as a wildcard for either component.
/// Examples: `"linux/amd64"`, `"darwin/arm64"`, `"*/*"`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    /// Returns the wildcard platform `*/*` that matches everything.
    pub fn any() -> Self {
        Self {
            os: "*".to_string(),
            arch: "*".to_string(),
        }
    }

    /// Returns the platform of the current system.
    ///
    /// Maps Rust's `std::env::consts` to Docker conventions:
    /// - `macos` → `darwin`
    /// - `x86_64` → `amd64`
    /// - `aarch64` → `arm64`
    pub fn current() -> Self {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        };
        Self {
            os: os.to_string(),
            arch: arch.to_string(),
        }
    }

    /// Check if this platform pattern matches a given os/arch pair.
    ///
    /// `*` in either component acts as a wildcard.
    pub fn matches(&self, os: &str, arch: &str) -> bool {
        (self.os == "*" || self.os == os) && (self.arch == "*" || self.arch == arch)
    }

    /// Check if this platform pattern matches the current system.
    pub fn matches_current(&self) -> bool {
        let current = Self::current();
        self.matches(&current.os, &current.arch)
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.os, self.arch)
    }
}

impl FromStr for Platform {
    type Err = BundlebaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(format!(
                "Invalid platform '{}'. Must be in os/arch format (e.g., 'linux/amd64', '*/*').",
                s
            )
            .into());
        }
        Ok(Self {
            os: parts[0].to_string(),
            arch: parts[1].to_string(),
        })
    }
}

impl From<Platform> for String {
    fn from(p: Platform) -> Self {
        p.to_string()
    }
}

impl TryFrom<String> for Platform {
    type Error = BundlebaseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// The execution environment for connector logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    Python,
    Lib,
    Java,
    Docker,
    Ipc,
}

impl fmt::Display for Runner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Runner::Python => write!(f, "python"),
            Runner::Lib => write!(f, "lib"),
            Runner::Java => write!(f, "java"),
            Runner::Docker => write!(f, "docker"),
            Runner::Ipc => write!(f, "ipc"),
        }
    }
}

impl FromStr for Runner {
    type Err = BundlebaseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "python" => Ok(Runner::Python),
            "lib" => Ok(Runner::Lib),
            "java" => Ok(Runner::Java),
            "docker" => Ok(Runner::Docker),
            "ipc" => Ok(Runner::Ipc),
            _ => Err(format!(
                "Invalid runner '{}'. Must be one of: python, lib, java, docker, ipc.",
                s
            )
            .into()),
        }
    }
}

/// A single connector entry binding a name+platform to runner+logic.
///
/// Multiple entries can exist for the same connector name (different platforms
/// or temporary vs persisted). Resolution picks the best match at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorEntry {
    pub name: NamespacedName,
    pub runner: Runner,
    pub logic: String,
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
        "No connector logic matches current platform '{}' for connector '{}'. Available platforms: {}",
        Platform::current(),
        name,
        platforms.join(", ")
    )
    .into())
}

/// Map a user-facing runner to the internal registry type ("native" or "ipc").
pub fn resolve_registry_type(runner: Runner) -> &'static str {
    match runner {
        Runner::Python | Runner::Lib => "native",
        Runner::Java | Runner::Docker | Runner::Ipc => "ipc",
    }
}

/// Reconstruct the prefixed call string from runner and logic for the native/ipc plugins.
pub fn build_call_string(runner: Runner, logic: &str) -> String {
    match runner {
        Runner::Python => format!("python:{}", logic),
        Runner::Lib => format!("lib:{}", logic),
        Runner::Java => format!("java:{}", logic),
        Runner::Docker => format!("docker:{}", logic),
        Runner::Ipc => logic.to_string(),
    }
}

/// Parse a FROM URL string like `runner://logic` into (Runner, logic).
///
/// The scheme (before `://`) is parsed as a Runner, and everything after `://` is the logic string.
///
/// # Examples
/// - `"ipc://./my_func"` → `(Runner::Ipc, "./my_func")`
/// - `"lib://./mylib.so"` → `(Runner::Lib, "./mylib.so")`
/// - `"ipc:///usr/bin/func"` → `(Runner::Ipc, "/usr/bin/func")`
/// - `"python://mod:func"` → `(Runner::Python, "mod:func")`
pub fn parse_from_url(from: &str) -> Result<(Runner, String), BundlebaseError> {
    let separator = "://";
    let pos = from.find(separator).ok_or_else(|| -> BundlebaseError {
        format!(
            "Invalid FROM URL '{}'. Expected format: 'runner://logic' (e.g., 'ipc://./my_func').",
            from
        )
        .into()
    })?;
    let scheme = &from[..pos];
    let logic = &from[pos + separator.len()..];
    if logic.is_empty() {
        return Err(format!(
            "Invalid FROM URL '{}'. Logic part after '://' cannot be empty.",
            from
        )
        .into());
    }
    let runner: Runner = scheme.parse()?;
    Ok((runner, logic.to_string()))
}

/// Format a runner and logic string into a FROM URL: `runner://logic`.
pub fn to_from_url(runner: Runner, logic: &str) -> String {
    format!("{}://{}", runner, logic)
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
    fn test_platform_from_str() {
        let p: Platform = "linux/amd64".parse().unwrap();
        assert_eq!(p.os, "linux");
        assert_eq!(p.arch, "amd64");
    }

    #[test]
    fn test_platform_from_str_invalid() {
        assert!("linux".parse::<Platform>().is_err());
        assert!("".parse::<Platform>().is_err());
        assert!("/amd64".parse::<Platform>().is_err());
        assert!("linux/".parse::<Platform>().is_err());
    }

    #[test]
    fn test_platform_display() {
        let p = Platform { os: "linux".to_string(), arch: "amd64".to_string() };
        assert_eq!(p.to_string(), "linux/amd64");
    }

    #[test]
    fn test_platform_any() {
        let p = Platform::any();
        assert_eq!(p.os, "*");
        assert_eq!(p.arch, "*");
        assert_eq!(p.to_string(), "*/*");
    }

    #[test]
    fn test_platform_matches_exact() {
        let p: Platform = "linux/amd64".parse().unwrap();
        assert!(p.matches("linux", "amd64"));
        assert!(!p.matches("darwin", "arm64"));
    }

    #[test]
    fn test_platform_matches_wildcard_all() {
        let p = Platform::any();
        assert!(p.matches("darwin", "arm64"));
        assert!(p.matches("linux", "amd64"));
    }

    #[test]
    fn test_platform_matches_wildcard_os() {
        let p: Platform = "*/amd64".parse().unwrap();
        assert!(p.matches("linux", "amd64"));
        assert!(p.matches("darwin", "amd64"));
        assert!(!p.matches("linux", "arm64"));
    }

    #[test]
    fn test_platform_matches_wildcard_arch() {
        let p: Platform = "linux/*".parse().unwrap();
        assert!(p.matches("linux", "amd64"));
        assert!(p.matches("linux", "arm64"));
        assert!(!p.matches("darwin", "arm64"));
    }

    #[test]
    fn test_platform_matches_current() {
        let p = Platform::any();
        assert!(p.matches_current());

        let p = Platform::current();
        assert!(p.matches_current());
    }

    #[test]
    fn test_platform_current() {
        let p = Platform::current();
        assert!(!p.os.is_empty());
        assert!(!p.arch.is_empty());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            assert_eq!(p.os, "darwin");
            assert_eq!(p.arch, "arm64");
        }
    }

    #[test]
    fn test_platform_serde_roundtrip() {
        let p: Platform = "linux/amd64".parse().unwrap();
        let yaml = serde_yaml_ng::to_string(&p).unwrap();
        assert!(yaml.contains("linux/amd64"));
        let deser: Platform = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(deser, p);
    }

    #[test]
    fn test_resolve_last_set_wins() {
        let entries = vec![
            ConnectorEntry {
                name: NamespacedName::new("test", "source"),
                runner: Runner::Lib,
                logic: "first".to_string(),
                platform: Platform::any(),
                temporary: false,
            },
            ConnectorEntry {
                name: NamespacedName::new("test", "source"),
                runner: Runner::Lib,
                logic: "second".to_string(),
                platform: Platform::any(),
                temporary: false,
            },
        ];

        let resolved = resolve_connector(&entries, "test.source").expect("should resolve");
        assert_eq!(resolved.logic, "second");
    }

    #[test]
    fn test_resolve_no_match() {
        let entries = vec![ConnectorEntry {
            name: NamespacedName::new("test", "source"),
            runner: Runner::Lib,
            logic: "test".to_string(),
            platform: "nonexistent/arch".parse().unwrap(),
            temporary: false,
        }];

        let result = resolve_connector(&entries, "test.source");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No connector logic matches"));
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
                name: NamespacedName::new("test", "source"),
                runner: Runner::Lib,
                logic: "persisted".to_string(),
                platform: Platform::any(),
                temporary: false,
            },
            ConnectorEntry {
                name: NamespacedName::new("test", "source"),
                runner: Runner::Python,
                logic: "temporary".to_string(),
                platform: Platform::any(),
                temporary: true,
            },
        ];

        let resolved = resolve_connector(&entries, "test.source").expect("should resolve");
        assert_eq!(resolved.logic, "temporary");
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

    #[test]
    fn test_parse_from_url_ipc_relative() {
        let (runner, logic) = parse_from_url("ipc://./my_func").unwrap();
        assert_eq!(runner, Runner::Ipc);
        assert_eq!(logic, "./my_func");
    }

    #[test]
    fn test_parse_from_url_ipc_absolute() {
        let (runner, logic) = parse_from_url("ipc:///usr/bin/func").unwrap();
        assert_eq!(runner, Runner::Ipc);
        assert_eq!(logic, "/usr/bin/func");
    }

    #[test]
    fn test_parse_from_url_lib() {
        let (runner, logic) = parse_from_url("lib://./mylib.so").unwrap();
        assert_eq!(runner, Runner::Lib);
        assert_eq!(logic, "./mylib.so");
    }

    #[test]
    fn test_parse_from_url_python() {
        let (runner, logic) = parse_from_url("python://mod:func").unwrap();
        assert_eq!(runner, Runner::Python);
        assert_eq!(logic, "mod:func");
    }

    #[test]
    fn test_parse_from_url_docker() {
        let (runner, logic) = parse_from_url("docker://my-image").unwrap();
        assert_eq!(runner, Runner::Docker);
        assert_eq!(logic, "my-image");
    }

    #[test]
    fn test_parse_from_url_java() {
        let (runner, logic) = parse_from_url("java://com.example.MyClass").unwrap();
        assert_eq!(runner, Runner::Java);
        assert_eq!(logic, "com.example.MyClass");
    }

    #[test]
    fn test_parse_from_url_invalid_no_separator() {
        assert!(parse_from_url("ipc:./my_func").is_err());
    }

    #[test]
    fn test_parse_from_url_invalid_empty_logic() {
        assert!(parse_from_url("ipc://").is_err());
    }

    #[test]
    fn test_parse_from_url_invalid_runner() {
        assert!(parse_from_url("unknown://./func").is_err());
    }

    #[test]
    fn test_to_from_url() {
        assert_eq!(to_from_url(Runner::Ipc, "./my_func"), "ipc://./my_func");
        assert_eq!(to_from_url(Runner::Lib, "./mylib.so"), "lib://./mylib.so");
        assert_eq!(to_from_url(Runner::Python, "mod:func"), "python://mod:func");
        assert_eq!(to_from_url(Runner::Ipc, "/usr/bin/func"), "ipc:///usr/bin/func");
    }

    #[test]
    fn test_from_url_roundtrip() {
        let url = "ipc://./my_func";
        let (runner, logic) = parse_from_url(url).unwrap();
        assert_eq!(to_from_url(runner, &logic), url);
    }
}
