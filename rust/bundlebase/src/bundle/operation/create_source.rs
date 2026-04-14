use crate::bundle::operation::Operation;
use crate::data::ObjectId;
use crate::object_id::ColumnId;
use crate::source::validate_connector_args;
use crate::{Bundle, BundlebaseError};
use arrow::datatypes::DataType;
use datafusion::error::DataFusionError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A column declared in the expected schema for a source.
///
/// `ExpectedColumn` entries are stored on `CreateSourceOp.expected_schema` and serve two purposes:
/// 1. Pre-reserve stable `ColumnId` values so that column operations (RENAME, CAST, etc.) can
///    reference columns by ID before any data is fetched into the bundle.
/// 2. Validate fetched data at `FETCH` time — warnings are emitted for missing, new, or
///    type-changed columns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExpectedColumn {
    /// Pre-reserved stable ID for this column. Used to match fetched columns by name.
    pub id: ColumnId,
    /// Column name (case-sensitive, must match fetched schema exactly).
    pub name: String,
    /// Expected Arrow data type.
    pub data_type: DataType,
}

/// Operation that creates a data source for a pack.
///
/// A source specifies where to look for data files and enables the `fetch()`
/// functionality to discover and auto-attach new files.
///
/// The connector is responsible for file discovery. Each connector may require
/// different arguments. For example, "remote_dir" requires:
/// - "url": Directory URL to list (e.g., "s3://bucket/data/")
/// - "patterns": Comma-separated glob patterns (e.g., "**/*.parquet,**/*.csv")
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

    /// How to save fetched data (auto, copy, parquet, ref). None = auto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_as: Option<String>,

    /// Optional batch size threshold in bytes. When set, small files fetched from
    /// this source are concatenated into batches until the total raw bytes exceeds
    /// this threshold, reducing per-file overhead. None = no batching.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "batchBytes"
    )]
    pub min_batch_bytes: Option<usize>,

    /// Optional expected schema for this source.
    ///
    /// When present, column IDs are pre-registered in BundleSchema on `apply()` so that
    /// column operations can reference them before any data is fetched. At fetch time,
    /// the fetched schema is compared against this expected schema and warnings are emitted
    /// for missing, new, or type-changed columns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_schema: Option<Vec<ExpectedColumn>>,
}

impl CreateSourceOp {
    pub fn setup(
        id: ObjectId,
        pack: ObjectId,
        connector: String,
        args: HashMap<String, String>,
        save_as: Option<String>,
    ) -> Self {
        Self {
            id,
            pack,
            connector,
            args,
            save_as,
            min_batch_bytes: None,
            expected_schema: None,
        }
    }
}

impl Operation for CreateSourceOp {
    fn describe(&self) -> String {
        let url = self
            .args
            .get("url")
            .map(|s| s.as_str())
            .unwrap_or("<no url>");
        format!(
            "CREATE SOURCE {} at {} for pack {}",
            self.id, url, self.pack
        )
    }

    fn to_hollow(&self, context: &super::HollowContext) -> Option<super::AnyOperation> {
        let expected = context.source_schemas.get(&self.id).cloned();
        let filled = CreateSourceOp {
            expected_schema: expected.map(|cols| {
                cols.into_iter()
                    .map(|(name, id, data_type)| ExpectedColumn {
                        id,
                        name,
                        data_type,
                    })
                    .collect()
            }),
            ..self.clone()
        };
        Some(super::AnyOperation::CreateSource(filled))
    }

    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError> {
        // min_batch_bytes only applies when fetched data is converted to parquet.
        // SAVE AS COPY keeps original bytes; SAVE AS REF doesn't store data at all.
        if self.min_batch_bytes.is_some() {
            if let Some(sa) = self.save_as.as_deref() {
                let lowered = sa.to_lowercase();
                if lowered == "copy" || lowered == "ref" {
                    return Err(format!(
                        "min_batch is not valid with save_as='{}'. \
                         Batching only applies when data is converted to parquet \
                         (save_as='auto' or save_as='parquet').",
                        lowered
                    )
                    .into());
                }
            }
        }

        // Verify pack exists
        if bundle.get_pack(&self.pack).is_none() {
            return Err(format!("Pack {} not found", self.pack).into());
        }

        if self.connector.contains('.') {
            // Dotted name: verify connector exists and resolves for current platform
            bundle
                .connector_registry()
                .read()
                .resolve_entry(&self.connector)?;
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
            save_as: None,
            min_batch_bytes: None,
            expected_schema: None,
        };

        assert_eq!(
            op.describe(),
            format!(
                "CREATE SOURCE {} at s3://bucket/data/ for pack {}",
                id, pack
            )
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
            save_as: None,
            min_batch_bytes: None,
            expected_schema: None,
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
            None,
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
            None,
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
            None,
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
            save_as: None,
            min_batch_bytes: None,
            expected_schema: None,
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

    #[tokio::test]
    async fn test_check_rejects_min_batch_bytes_with_save_as_copy() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = CreateSourceOp {
            id: ObjectId::generate(),
            pack: ObjectId::BASE_PACK,
            connector: "remote_dir".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.jsonl")),
            save_as: Some("copy".to_string()),
            min_batch_bytes: Some(1024 * 1024),
            expected_schema: None,
        };
        let err = op.check(&bundle).await.expect_err("should reject");
        let msg = err.to_string();
        assert!(msg.contains("min_batch"), "msg: {}", msg);
        assert!(msg.contains("copy"), "msg: {}", msg);
    }

    #[tokio::test]
    async fn test_check_rejects_min_batch_bytes_with_save_as_ref() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        let op = CreateSourceOp {
            id: ObjectId::generate(),
            pack: ObjectId::BASE_PACK,
            connector: "remote_dir".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.jsonl")),
            save_as: Some("ref".to_string()),
            min_batch_bytes: Some(1024 * 1024),
            expected_schema: None,
        };
        let err = op.check(&bundle).await.expect_err("should reject");
        let msg = err.to_string();
        assert!(msg.contains("min_batch"), "msg: {}", msg);
        assert!(msg.contains("ref"), "msg: {}", msg);
    }

    #[tokio::test]
    async fn test_check_allows_min_batch_bytes_with_save_as_parquet() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_pack(
            ObjectId::BASE_PACK,
            std::sync::Arc::new(crate::bundle::pack::Pack::new_base()),
        );
        let op = CreateSourceOp {
            id: ObjectId::generate(),
            pack: ObjectId::BASE_PACK,
            connector: "remote_dir".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.jsonl")),
            save_as: Some("parquet".to_string()),
            min_batch_bytes: Some(1024 * 1024),
            expected_schema: None,
        };
        op.check(&bundle)
            .await
            .expect("should allow parquet + batch");
    }

    #[tokio::test]
    async fn test_check_allows_min_batch_bytes_with_save_as_auto() {
        let bundle = Bundle::empty(None).await.expect("empty bundle");
        bundle.add_pack(
            ObjectId::BASE_PACK,
            std::sync::Arc::new(crate::bundle::pack::Pack::new_base()),
        );
        let op = CreateSourceOp {
            id: ObjectId::generate(),
            pack: ObjectId::BASE_PACK,
            connector: "remote_dir".to_string(),
            args: make_args("s3://bucket/data/", Some("**/*.jsonl")),
            save_as: None,
            min_batch_bytes: Some(1024 * 1024),
            expected_schema: None,
        };
        op.check(&bundle).await.expect("should allow auto + batch");
    }
}
