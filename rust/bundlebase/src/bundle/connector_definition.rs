//! Connector definition system for named, platform-aware connector logic.
//!
//! A `ConnectorDefinition` is created via `CREATE CONNECTOR acme.datasources.weather`
//! and holds one or more `ConnectorLogicEntry` values, each targeting a platform.
//! `create_source` resolves the definition to the current platform at runtime.

use crate::BundlebaseError;
use parking_lot::RwLock;

/// A named connector definition that can have multiple platform-specific logic entries.
#[derive(Debug)]
pub struct ConnectorDefinition {
    /// Full dotted name (e.g., "acme.datasources.weather")
    pub name: String,
    /// Platform-specific logic entries (last-set wins for overlapping platforms)
    logic_entries: RwLock<Vec<ConnectorLogicEntry>>,
}

/// A single platform-specific implementation for a connector definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorLogicEntry {
    /// Source type: "python", "lib", "java", "docker", or "ipc"
    pub source_type: String,
    /// Logic string (e.g., "mod:Class" for python, "./lib.so" for lib, "./my_source" for ipc)
    pub logic: String,
    /// Platform pattern in Docker-style os/arch (e.g., "linux/amd64", "*/*")
    pub platform: String,
}

/// Valid source type values for connector logic.
pub const VALID_SOURCE_TYPES: &[&str] = &["python", "lib", "java", "docker", "ipc"];

/// Map a user-facing source type to the internal registry type ("native" or "ipc").
pub fn resolve_registry_type(source_type: &str) -> Result<&'static str, BundlebaseError> {
    match source_type {
        "python" | "lib" => Ok("native"),
        "java" | "docker" | "ipc" => Ok("ipc"),
        _ => Err(format!(
            "Invalid source type '{}'. Must be one of: {}.",
            source_type,
            VALID_SOURCE_TYPES.join(", ")
        )
        .into()),
    }
}

/// Reconstruct the prefixed call string from source type and logic for the native/ipc plugins.
pub fn build_call_string(source_type: &str, logic: &str) -> String {
    match source_type {
        "python" => format!("python:{}", logic),
        "lib" => format!("lib:{}", logic),
        "java" => format!("java:{}", logic),
        "docker" => format!("docker:{}", logic),
        "ipc" => logic.to_string(),
        _ => logic.to_string(),
    }
}

impl ConnectorDefinition {
    /// Create a new empty connector definition.
    pub fn new(name: String) -> Self {
        Self {
            name,
            logic_entries: RwLock::new(Vec::new()),
        }
    }

    /// Add a logic entry. Last-set wins for overlapping platforms.
    pub fn add_logic(&self, entry: ConnectorLogicEntry) {
        self.logic_entries.write().push(entry);
    }

    /// Remove all logic entries. Returns the number of entries removed.
    pub fn remove_all_logic(&self) -> usize {
        let mut entries = self.logic_entries.write();
        let count = entries.len();
        entries.clear();
        count
    }

    /// Remove logic entries matching a specific platform. Returns the number removed.
    pub fn remove_logic_for_platform(&self, platform: &str) -> usize {
        let mut entries = self.logic_entries.write();
        let before = entries.len();
        entries.retain(|e| e.platform != platform);
        before - entries.len()
    }

    /// Resolve the best logic entry for the current platform.
    /// Iterates entries in reverse (last-set wins), returns first match.
    pub fn resolve_logic(&self) -> Result<ConnectorLogicEntry, BundlebaseError> {
        let (os, arch) = current_platform();
        let entries = self.logic_entries.read();

        for entry in entries.iter().rev() {
            if matches_platform(&entry.platform, &os, &arch) {
                return Ok(entry.clone());
            }
        }

        Err(format!(
            "No connector logic matches current platform '{}/{}' for connector '{}'",
            os, arch, self.name
        )
        .into())
    }
}

/// Check if a platform pattern matches a given os/arch pair.
///
/// Pattern uses Docker-style `os/arch` with `*` as wildcard.
/// Examples: `"linux/amd64"`, `"*/amd64"`, `"linux/*"`, `"*/*"`
pub fn matches_platform(pattern: &str, os: &str, arch: &str) -> bool {
    let parts: Vec<&str> = pattern.split('/').collect();
    if parts.len() != 2 {
        return false;
    }
    let (pat_os, pat_arch) = (parts[0], parts[1]);
    (pat_os == "*" || pat_os == os) && (pat_arch == "*" || pat_arch == arch)
}

/// Return the current platform as Docker-style (os, arch).
///
/// Maps Rust's `std::env::consts` to Docker conventions:
/// - `macos` → `darwin`
/// - `x86_64` → `amd64`
/// - `aarch64` → `arm64`
pub fn current_platform() -> (String, String) {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    (os.to_string(), arch.to_string())
}

/// Parse a dotted connector name into (namespace, name).
///
/// The name must contain at least one dot. The last segment is the name,
/// everything before is the namespace.
///
/// # Examples
/// - `"acme.datasources.weather"` → `("acme.datasources", "weather")`
/// - `"acme.weather"` → `("acme", "weather")`
/// - `"weather"` → error
pub fn parse_connector_name(name: &str) -> Result<(&str, &str), BundlebaseError> {
    match name.rfind('.') {
        Some(pos) => {
            let namespace = &name[..pos];
            let short_name = &name[pos + 1..];
            if namespace.is_empty() || short_name.is_empty() {
                return Err(format!(
                    "Invalid connector name '{}': namespace and name must not be empty",
                    name
                )
                .into());
            }
            Ok((namespace, short_name))
        }
        None => Err(format!(
            "Connector name '{}' must contain at least one dot (e.g., 'acme.weather')",
            name
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_platform_exact() {
        assert!(matches_platform("linux/amd64", "linux", "amd64"));
    }

    #[test]
    fn test_matches_platform_wildcard_all() {
        assert!(matches_platform("*/*", "darwin", "arm64"));
        assert!(matches_platform("*/*", "linux", "amd64"));
    }

    #[test]
    fn test_matches_platform_wildcard_os() {
        assert!(matches_platform("*/amd64", "linux", "amd64"));
        assert!(matches_platform("*/amd64", "darwin", "amd64"));
        assert!(!matches_platform("*/amd64", "linux", "arm64"));
    }

    #[test]
    fn test_matches_platform_wildcard_arch() {
        assert!(matches_platform("linux/*", "linux", "amd64"));
        assert!(matches_platform("linux/*", "linux", "arm64"));
        assert!(!matches_platform("linux/*", "darwin", "arm64"));
    }

    #[test]
    fn test_matches_platform_no_match() {
        assert!(!matches_platform("linux/amd64", "darwin", "arm64"));
    }

    #[test]
    fn test_matches_platform_invalid_format() {
        assert!(!matches_platform("linux", "linux", "amd64"));
        assert!(!matches_platform("", "linux", "amd64"));
    }

    #[test]
    fn test_current_platform() {
        let (os, arch) = current_platform();
        assert!(!os.is_empty());
        assert!(!arch.is_empty());
        // On macOS ARM, should be "darwin" and "arm64"
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            assert_eq!(os, "darwin");
            assert_eq!(arch, "arm64");
        }
    }

    #[test]
    fn test_resolve_last_set_wins() {
        let def = ConnectorDefinition::new("test.source".to_string());
        def.add_logic(ConnectorLogicEntry {
            source_type: "lib".to_string(),
            logic: "first".to_string(),
            platform: "*/*".to_string(),
        });
        def.add_logic(ConnectorLogicEntry {
            source_type: "lib".to_string(),
            logic: "second".to_string(),
            platform: "*/*".to_string(),
        });

        let resolved = def.resolve_logic().expect("should resolve");
        assert_eq!(resolved.logic, "second");
    }

    #[test]
    fn test_resolve_no_match() {
        let def = ConnectorDefinition::new("test.source".to_string());
        def.add_logic(ConnectorLogicEntry {
            source_type: "lib".to_string(),
            logic: "test".to_string(),
            platform: "nonexistent/arch".to_string(),
        });

        let result = def.resolve_logic();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No connector logic matches"));
    }

    #[test]
    fn test_parse_connector_name_valid() {
        let (ns, name) = parse_connector_name("acme.datasources.weather").unwrap();
        assert_eq!(ns, "acme.datasources");
        assert_eq!(name, "weather");
    }

    #[test]
    fn test_parse_connector_name_no_dot() {
        let result = parse_connector_name("weather");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must contain at least one dot"));
    }

    #[test]
    fn test_parse_connector_name_single_dot() {
        let (ns, name) = parse_connector_name("acme.weather").unwrap();
        assert_eq!(ns, "acme");
        assert_eq!(name, "weather");
    }

    #[test]
    fn test_remove_all_logic() {
        let def = ConnectorDefinition::new("test.source".to_string());
        def.add_logic(ConnectorLogicEntry {
            source_type: "lib".to_string(),
            logic: "first".to_string(),
            platform: "*/*".to_string(),
        });
        def.add_logic(ConnectorLogicEntry {
            source_type: "lib".to_string(),
            logic: "second".to_string(),
            platform: "linux/amd64".to_string(),
        });

        let removed = def.remove_all_logic();
        assert_eq!(removed, 2);
        assert!(def.resolve_logic().is_err());
    }

    #[test]
    fn test_remove_logic_for_platform() {
        let def = ConnectorDefinition::new("test.source".to_string());
        def.add_logic(ConnectorLogicEntry {
            source_type: "lib".to_string(),
            logic: "wildcard".to_string(),
            platform: "*/*".to_string(),
        });
        def.add_logic(ConnectorLogicEntry {
            source_type: "lib".to_string(),
            logic: "linux-only".to_string(),
            platform: "linux/amd64".to_string(),
        });

        let removed = def.remove_logic_for_platform("linux/amd64");
        assert_eq!(removed, 1);

        let resolved = def.resolve_logic().expect("should resolve wildcard");
        assert_eq!(resolved.logic, "wildcard");
    }

    #[test]
    fn test_remove_logic_for_platform_no_match() {
        let def = ConnectorDefinition::new("test.source".to_string());
        def.add_logic(ConnectorLogicEntry {
            source_type: "lib".to_string(),
            logic: "test".to_string(),
            platform: "*/*".to_string(),
        });

        let removed = def.remove_logic_for_platform("linux/amd64");
        assert_eq!(removed, 0);
    }
}
