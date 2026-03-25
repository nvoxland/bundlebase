use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Computes a consistent SHA256 hash of a serializable object.
/// Keys are sorted alphabetically to ensure consistency.
pub fn hash_config<T: Serialize>(config: &T) -> String {
    // Serialize to a serde_json value to sort keys
    let value = serde_json::to_value(config).expect("BUG: config must be serializable");

    // Convert to a string with sorted keys
    let sorted_yaml = sort_json_keys(&value);

    // Hash the sorted JSON
    let mut hasher = Sha256::new();
    hasher.update(sorted_yaml.as_bytes());
    let hash = hasher.finalize();

    hex::encode(hash)[0..12].to_string()
}

/// Recursively sorts JSON object keys alphabetically
fn sort_json_keys(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            // Create a BTreeMap to sort keys
            let sorted: BTreeMap<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_json_keys(v).parse().unwrap_or(v.clone())))
                .collect();

            // Rebuild by recursively sorting nested objects
            let sorted_again: BTreeMap<String, serde_json::Value> = sorted
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        match v {
                            serde_json::Value::Object(_) => {
                                serde_json::from_str(&sort_json_keys(&v)).unwrap_or(v)
                            }
                            _ => v,
                        },
                    )
                })
                .collect();

            serde_json::to_string(&sorted_again).unwrap_or_else(|_| "{}".to_string())
        }
        serde_json::Value::Array(arr) => {
            let sorted: Vec<serde_json::Value> = arr
                .iter()
                .map(|v| {
                    if let serde_json::Value::Object(_) = v {
                        serde_json::from_str(&sort_json_keys(v)).unwrap_or_else(|_| v.clone())
                    } else {
                        v.clone()
                    }
                })
                .collect();
            serde_json::to_string(&sorted).unwrap_or_else(|_| "[]".to_string())
        }
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct TestConfig {
        name: String,
        value: i32,
    }

    #[test]
    fn test_hash_config_consistency() {
        let config = TestConfig {
            name: "test".to_string(),
            value: 42,
        };

        let hash1 = hash_config(&config);
        let hash2 = hash_config(&config);

        assert_eq!(hash1, hash2, "Same config should produce same hash");
    }

    #[test]
    fn test_hash_config_deterministic() {
        // Create two configs with same data but constructed differently
        let config1 = TestConfig {
            name: "test".to_string(),
            value: 42,
        };

        let config2 = TestConfig {
            value: 42,
            name: "test".to_string(),
        };

        let hash1 = hash_config(&config1);
        let hash2 = hash_config(&config2);

        assert_eq!(
            hash1, hash2,
            "Configs with same data should produce same hash"
        );
    }

    #[test]
    fn test_hash_config_sensitivity() {
        let config1 = TestConfig {
            name: "test".to_string(),
            value: 42,
        };

        let config2 = TestConfig {
            name: "test".to_string(),
            value: 43,
        };

        let hash1 = hash_config(&config1);
        let hash2 = hash_config(&config2);

        assert_ne!(
            hash1, hash2,
            "Different configs should produce different hashes"
        );
    }

    #[test]
    fn test_sorted_keys() {
        let json_str = r#"{"z": 1, "a": 2, "m": 3}"#;
        let value: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let sorted = sort_json_keys(&value);

        // The sorted JSON should have keys in alphabetical order
        let sorted_value: serde_json::Value = serde_json::from_str(&sorted).unwrap();
        let keys: Vec<_> = sorted_value.as_object().unwrap().keys().collect();

        assert_eq!(
            keys,
            vec!["a", "m", "z"],
            "Keys should be sorted alphabetically"
        );
    }
}
