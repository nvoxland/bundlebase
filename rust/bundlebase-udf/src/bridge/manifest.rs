//! Shared manifest types for function discovery across all runtimes.
//!
//! Every runtime (FFI, IPC, Java, Python, Docker) uses the same `Manifest`
//! and `ManifestEntry` structs to describe discovered functions.

use serde::Deserialize;

/// A single function entry from a manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct ManifestEntry {
    pub name: String,
    #[serde(default)]
    pub symbol: Option<String>,
    pub input_types: Vec<String>,
    pub return_type: String,
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "scalar".to_string()
}

/// JSON manifest returned by `bundlebase_functions()`.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub functions: Vec<ManifestEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_deserialize() {
        let json = r#"{"functions": [
            {"name": "double_val", "symbol": "double_val",
             "input_types": ["Int64"], "return_type": "Int64", "kind": "scalar"},
            {"name": "my_sum", "input_types": ["Int64"],
             "return_type": "Int64", "kind": "aggregate"}
        ]}"#;

        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.functions.len(), 2);

        assert_eq!(manifest.functions[0].name, "double_val");
        assert_eq!(manifest.functions[0].symbol, Some("double_val".to_string()));
        assert_eq!(manifest.functions[0].input_types, vec!["Int64"]);
        assert_eq!(manifest.functions[0].return_type, "Int64");
        assert_eq!(manifest.functions[0].kind, "scalar");

        assert_eq!(manifest.functions[1].name, "my_sum");
        assert_eq!(manifest.functions[1].symbol, None);
        assert_eq!(manifest.functions[1].kind, "aggregate");
    }

    #[test]
    fn test_manifest_default_kind() {
        let json = r#"{"functions": [
            {"name": "double_val", "input_types": ["Int64"], "return_type": "Int64"}
        ]}"#;

        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.functions[0].kind, "scalar");
    }

    #[test]
    fn test_manifest_multi_input() {
        let json = r#"{"functions": [
            {"name": "add", "input_types": ["Int64", "Int64"], "return_type": "Int64"}
        ]}"#;

        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.functions[0].input_types, vec!["Int64", "Int64"]);
    }
}
