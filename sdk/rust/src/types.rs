use serde::{Deserialize, Serialize};

/// Represents a discovered data location returned from [`Connector::discover`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub location: String,
    #[serde(default = "default_must_copy")]
    pub must_copy: bool,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub version: String,
}

fn default_must_copy() -> bool {
    true
}

fn default_format() -> String {
    "parquet".to_string()
}

impl Location {
    pub fn new(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            must_copy: true,
            format: "parquet".to_string(),
            version: String::new(),
        }
    }
}

/// Represents a stable URL for a data location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StableUrl {
    pub url: String,
}
