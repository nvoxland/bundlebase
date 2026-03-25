use crate::bundle::operation::Operation;
use crate::data::ObjectId;
use crate::source::validate_connector_args;
use crate::{Bundle, BundlebaseError};
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Operation that creates a data source for a pack.
///
/// A source specifies where to look for data files and enables the `fetch()`
/// functionality to discover and auto-attach new files.
///
/// The connector is responsible for file discovery. Each connector may require
/// different arguments. For example, "remote_dir" requires:
/// - "url": Directory URL to list (e.g., "s3://bucket/data/")
/// - "patterns": Comma-separated glob patterns (e.g., "**/*.parquet,**/*.csv")
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateSourceOp {
    /// Unique identifier for this source
    pub id: ObjectId,

    /// The pack this source is associated with
    pub pack: ObjectId,

    /// Connector name (e.g., "remote_dir" for built-in, or "acme.weather" for custom)
    pub connector: String,

    /// Connector-specific configuration arguments.
    /// For "remote_dir":
    /// - "url": Directory URL (required)
    /// - "patterns": Comma-separated glob patterns (optional, defaults to "**/*")
    #[serde(default)]
    pub args: HashMap<String, String>,
}

impl CreateSourceOp {
    pub fn setup(
        id: ObjectId,
        pack: ObjectId,
        connector: String,
        args: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            pack,
            connector,
            args,
        }
    }
}

impl Operation for CreateSourceOp {
    fn describe(&self) -> String {
        let url = self.args.get("url").map(|s| s.as_str()).unwrap_or("<no url>");
        format!("CREATE SOURCE {} at {} for pack {}", self.id, url, self.pack)
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // Verify pack exists
        if bundle.get_pack(&self.pack).is_none() {
            return Err(format!("Pack {} not found", self.pack).into());
        }

        if self.connector.contains('.') {
            // Dotted name: verify connector exists and resolves for current platform
            bundle.connector_registry().read().resolve_entry(&self.connector)?;
        } else {
            // Built-in function: look up in registry
            let registry = bundle.connector_registry();
            let registry_guard = registry.read();
            let func = registry_guard.get(&self.connector).ok_or_else(|| {
                let available = registry_guard.connector_names();
                format!(
                    "Unknown connector '{}'. Available connectors: {}",
                    self.connector,
                    available.join(", ")
                )
            })?;

            validate_connector_args(func.as_ref(), &self.args)?;
        }

        Ok(())
    }

    fn allowed_on_view(&self) -> bool {
        false
    }

    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError> {
        bundle.add_source(self.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_args(url: &str, patterns: Option<&str>) -> HashMap<String, String> {
        let mut args = HashMap::new();
        args.insert("url".to_string(), url.to_string());
        if let Some(p) = patterns {
            args.insert("patterns".to_string(), p.to_string());
        }
        args
    }

    #[test]
    fn test_describe() {
        let id = ObjectId::generate();
        let pack = ObjectId::generate();
        let op = CreateSourceOp {
            id,
            pack,
            connector: "remote_dir".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.parquet")),
        };

        assert_eq!(
            op.describe(),
            format!("CREATE SOURCE {} at s3://bucket/data/ for pack {}", id, pack)
        );
    }

    #[test]
    fn test_describe_no_url() {
        let id = ObjectId::generate();
        let pack = ObjectId::generate();
        let op = CreateSourceOp {
            id,
            pack,
            connector: "custom_function".to_string(),
            args: HashMap::new(),
        };

        assert_eq!(
            op.describe(),
            format!("CREATE SOURCE {} at <no url> for pack {}", id, pack)
        );
    }

    #[test]
    fn test_setup() {
        let op = CreateSourceOp::setup(
            ObjectId::generate(),
            ObjectId::generate(),
            "remote_dir".to_string(),
            make_args("s3://bucket/", None),
        );

        assert_eq!(op.connector, "remote_dir");
        assert_eq!(op.args.get("url"), Some(&"s3://bucket/".to_string()));
    }

    #[test]
    fn test_setup_with_patterns() {
        let op = CreateSourceOp::setup(
            ObjectId::generate(),
            ObjectId::generate(),
            "remote_dir".to_string(),
            make_args("s3://bucket/", Some("**/*.parquet,**/*.csv")),
        );

        assert_eq!(op.connector, "remote_dir");
        assert_eq!(
            op.args.get("patterns"),
            Some(&"**/*.parquet,**/*.csv".to_string())
        );
    }

    #[test]
    fn test_setup_with_extra_args() {
        let mut args = make_args("s3://bucket/", Some("**/*"));
        args.insert("key".to_string(), "value".to_string());

        let op = CreateSourceOp::setup(
            ObjectId::generate(),
            ObjectId::generate(),
            "custom_function".to_string(),
            args.clone(),
        );

        assert_eq!(op.connector, "custom_function");
        assert_eq!(op.args, args);
    }

    #[test]
    fn test_serialization() {
        let id = ObjectId::generate();
        let pack = ObjectId::generate();
        let op = CreateSourceOp {
            id,
            pack,
            connector: "remote_dir".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.parquet")),
        };

        let yaml = serde_yaml_ng::to_string(&op).unwrap();
        let id_str: String = id.into();
        let pack_str: String = pack.into();
        assert!(yaml.contains(&id_str));
        assert!(yaml.contains(&pack_str));
        assert!(yaml.contains("connector: remote_dir"));
        assert!(yaml.contains("url: s3://bucket/data/"));
        assert!(yaml.contains("patterns: '**/*.parquet'"));
    }
}
