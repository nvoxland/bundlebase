use arrow::array::{Int32Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use std::sync::Arc;

use crate::bundle::operation::{AnyOperation, BundleChange};
use crate::BundlebaseError;
use bundlebase_common::command_response::{single_batch_stream, CommandResponse, OutputShape};
use bundlebase_common::impl_dyn_command_response;
use datafusion::execution::SendableRecordBatchStream;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BundleCommit {
    #[serde(skip)]
    pub url: Option<Url>,
    #[serde(skip)]
    pub data_dir: Option<Url>,
    pub author: String,
    pub message: String,
    pub timestamp: String,
    pub changes: Vec<BundleChange>,
}

impl BundleCommit {
    /// Convenience method to get all operations as a flat list
    pub fn operations(&self) -> Vec<AnyOperation> {
        self.changes
            .iter()
            .flat_map(|change| change.operations.clone())
            .collect()
    }
}

/// Newtype wrapper around `Vec<BundleCommit>` for `CommandResponse` implementation.
///
/// This wrapper exists to satisfy Rust's orphan rules — `CommandResponse` is defined
/// in `bundlebase_common`, and `Vec<T>` is foreign, so the impl must be on a local type.
pub struct CommitHistory(pub Vec<BundleCommit>);

impl std::ops::Deref for CommitHistory {
    type Target = Vec<BundleCommit>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<BundleCommit>> for CommitHistory {
    fn from(commits: Vec<BundleCommit>) -> Self {
        CommitHistory(commits)
    }
}

/// CommandResponse implementation for displaying commit history.
impl CommandResponse for CommitHistory {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("url", DataType::Utf8, true),
            Field::new("author", DataType::Utf8, false),
            Field::new("message", DataType::Utf8, false),
            Field::new("timestamp", DataType::Utf8, false),
            Field::new("change_count", DataType::Int32, false),
        ]))
    }

    fn output_shape() -> OutputShape {
        OutputShape::Table
    }

    fn into_stream(self: Box<Self>) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let ids: Vec<i32> = (0..self.0.len() as i32).collect();
        let urls: Vec<Option<String>> = self
            .0
            .iter()
            .map(|c| c.url.as_ref().map(|u| u.to_string()))
            .collect();
        let authors: Vec<&str> = self.0.iter().map(|c| c.author.as_str()).collect();
        let messages: Vec<&str> = self.0.iter().map(|c| c.message.as_str()).collect();
        let timestamps: Vec<&str> = self.0.iter().map(|c| c.timestamp.as_str()).collect();
        let change_counts: Vec<i32> = self.0.iter().map(|c| c.changes.len() as i32).collect();

        let batch = RecordBatch::try_new(
            Self::schema(),
            vec![
                Arc::new(Int32Array::from(ids)),
                Arc::new(StringArray::from(urls)),
                Arc::new(StringArray::from(authors)),
                Arc::new(StringArray::from(messages)),
                Arc::new(StringArray::from(timestamps)),
                Arc::new(Int32Array::from(change_counts)),
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;
        single_batch_stream(Self::schema(), batch)
    }

    impl_dyn_command_response!(CommitHistory);
}

/// Extracts the version number from a manifest filename.
/// Expected format: `{5-digit-version}{12-char-hash}.yaml`
/// Examples: "00001abc123def456.yaml" -> 1, "00042xyz789abc123.yaml" -> 42
pub fn manifest_version(filename: &str) -> u32 {
    if filename.len() < 5 {
        return 1; // Default to version 1 for malformed filenames
    }

    filename[0..5].parse::<u32>().unwrap_or(1) // Default to version 1 if parsing fails
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::operation::{
        BundleChange, DropColumnOp, RenameColumnOp, SetDescriptionOp, SetNameOp,
    };
    use crate::object_id::ColumnId;
    use uuid::Uuid;

    // Helper function to create a test UUID
    fn test_uuid() -> Uuid {
        Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap()
    }

    #[test]
    fn test_serialize_empty_operations() {
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Initial commit".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![],
        };
        let yaml = serde_yaml_ng::to_string(&commit).unwrap();

        let expected = r"author: test-user
message: Initial commit
timestamp: 2024-01-01T00:00:00Z
changes: []
";
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_single_operation() {
        let id1 = ColumnId::generate();
        let op = DropColumnOp::setup(id1);
        let change = BundleChange {
            id: test_uuid(),
            description: "Remove columns".to_string(),
            operations: vec![op.into()],
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Remove column".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };
        let yaml = serde_yaml_ng::to_string(&commit).unwrap();

        let expected = format!(
            r"author: test-user
message: Remove column
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Remove columns
  operations:
  - type: dropColumn
    id: {}
",
            id1
        );
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_multiple_operations() {
        let op1 = SetNameOp::setup("Test");
        let drop_id = ColumnId::generate();
        let op2 = DropColumnOp::setup(drop_id);
        let rename_id = ColumnId::generate();
        let op3 = RenameColumnOp::setup(rename_id, "new");

        let change = BundleChange {
            id: test_uuid(),
            description: "Multiple operations".to_string(),
            operations: vec![op1.into(), op2.into(), op3.into()],
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Multiple ops".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };
        let yaml = serde_yaml_ng::to_string(&commit).unwrap();

        let expected = format!(
            r"author: test-user
message: Multiple ops
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Multiple operations
  operations:
  - type: setName
    name: Test
  - type: dropColumn
    id: {drop_id}
  - type: renameColumn
    id: {rename_id}
    newName: new
"
        );
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_with_from() {
        let op = SetNameOp::setup("Test");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set name".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Extended commit".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };
        let yaml = serde_yaml_ng::to_string(&commit).unwrap();

        let expected = r"
author: test-user
message: Extended commit
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Set name
  operations:
  - type: setName
    name: Test
";
        assert_eq!(yaml.trim(), expected.trim());
    }

    #[test]
    fn test_deserialize_empty_operations() {
        let yaml = r"author: test-user
message: Initial commit
timestamp: '2024-01-01T00:00:00Z'
changes: []
";
        let commit: BundleCommit = serde_yaml_ng::from_str(yaml).unwrap();

        assert_eq!(commit.message, "Initial commit");
        assert_eq!(commit.author, "test-user");
        assert_eq!(commit.timestamp, "2024-01-01T00:00:00Z");
        assert_eq!(commit.changes.len(), 0);
    }

    #[test]
    fn test_deserialize_with_from() {
        let yaml = r"
author: test-user
message: Extended
timestamp: '2024-01-01T00:00:00Z'
changes: []
";
        let commit: BundleCommit = serde_yaml_ng::from_str(yaml).unwrap();

        assert_eq!(commit.message, "Extended");
        assert_eq!(commit.author, "test-user");
        assert_eq!(commit.timestamp, "2024-01-01T00:00:00Z");
    }

    #[test]
    fn test_deserialize_multiple_operations() {
        let yaml = r"author: test-user
message: Multiple ops
timestamp: '2024-01-01T00:00:00Z'
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Multiple operations
  operations:
  - type: setName
    name: Test
  - type: dropColumn
    id: '0000000000000001'
    name: col1
  - type: renameColumn
    id: '0000000000000002'
    oldName: old
    newName: new
";
        let commit: BundleCommit = serde_yaml_ng::from_str(yaml).unwrap();

        assert_eq!(commit.message, "Multiple ops");
        assert_eq!(commit.author, "test-user");
        assert_eq!(commit.timestamp, "2024-01-01T00:00:00Z");
        assert_eq!(commit.changes.len(), 1);
        assert_eq!(commit.changes[0].operations.len(), 3);

        assert_eq!(
            commit.changes[0].operations[0],
            AnyOperation::SetName(SetNameOp {
                name: "Test".to_string(),
            })
        );
    }

    #[test]
    fn test_serialize_camel_case_conversion() {
        // Test that camelCase conversion happens for all field names
        let col_id = ColumnId::generate();
        let op = RenameColumnOp::setup(col_id, "first_name");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Rename column".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Test camelCase".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };
        let yaml = serde_yaml_ng::to_string(&commit).unwrap();

        // Should have newName in camelCase
        let expected = format!(
            r"author: test-user
message: Test camelCase
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Rename column
  operations:
  - type: renameColumn
    id: {col_id}
    newName: first_name
"
        );
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_type_always_first() {
        // Verify that "type" field is always added first in the mapping
        let col_id = ColumnId::generate();
        let op = RenameColumnOp::setup(col_id, "b");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Rename".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Test".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };
        let yaml = serde_yaml_ng::to_string(&commit).unwrap();

        // Find the operations section within changes and verify type comes first
        let operations_start = yaml.find("operations:").unwrap();
        let operations_section = &yaml[operations_start..];
        let first_line_after_dash = operations_section.find("- type:").unwrap();

        // There should be "- type:" right after the operations: line
        assert!(first_line_after_dash > 0);

        // Verify the exact order
        let expected = format!(
            r"author: test-user
message: Test
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Rename
  operations:
  - type: renameColumn
    id: {col_id}
    newName: b
"
        );
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_set_name() {
        let op = SetNameOp::setup("My Bundle");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set name".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Set bundle name".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };
        let yaml = serde_yaml_ng::to_string(&commit).unwrap();

        let expected = r"author: test-user
message: Set bundle name
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Set name
  operations:
  - type: setName
    name: My Bundle
";
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_set_description() {
        let op = SetDescriptionOp::setup("This is a test bundle");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set description".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Set description".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };
        let yaml = serde_yaml_ng::to_string(&commit).unwrap();

        let expected = r"author: test-user
message: Set description
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Set description
  operations:
  - type: setDescription
    description: This is a test bundle
";
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_special_characters_in_message() {
        let op = SetNameOp::setup("Test");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set name".to_string(),
        suppress_auto_reindex: false,
        };
        let message = "Commit with special chars: !@#$%".to_string();
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: message.clone(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };

        let yaml = serde_yaml_ng::to_string(&commit).unwrap();
        let deserialized: BundleCommit = serde_yaml_ng::from_str(&yaml).unwrap();

        assert_eq!(deserialized.message, message);
        assert_eq!(deserialized.author, "test-user");
        assert_eq!(deserialized.timestamp, "2024-01-01T00:00:00Z");
    }

    #[test]
    fn test_serialize_special_characters_in_names() {
        let col_id = ColumnId::generate();
        let op = RenameColumnOp::setup(col_id, "col-with-dash");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Rename".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Rename".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };

        let yaml = serde_yaml_ng::to_string(&commit).unwrap();
        let expected = format!(
            r"author: test-user
message: Rename
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Rename
  operations:
  - type: renameColumn
    id: {col_id}
    newName: col-with-dash
"
        );
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_empty_string_values() {
        let op = SetNameOp::setup("");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set name".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };

        let yaml = serde_yaml_ng::to_string(&commit).unwrap();
        let deserialized: BundleCommit = serde_yaml_ng::from_str(&yaml).unwrap();

        assert_eq!(deserialized.message, "");
        assert_eq!(deserialized.author, "test-user");
        assert_eq!(deserialized.timestamp, "2024-01-01T00:00:00Z");
        assert_eq!(deserialized.operations().len(), 1);
    }

    #[test]
    fn test_serialize_long_message() {
        let long_message = "A".repeat(1000);
        let op = SetNameOp::setup("Test");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set name".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: long_message.clone(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };

        let yaml = serde_yaml_ng::to_string(&commit).unwrap();
        let deserialized: BundleCommit = serde_yaml_ng::from_str(&yaml).unwrap();

        assert_eq!(deserialized.message, long_message);
        assert_eq!(deserialized.author, "test-user");
        assert_eq!(deserialized.timestamp, "2024-01-01T00:00:00Z");
    }

    #[test]
    fn test_serialize_unicode_characters() {
        let op = SetDescriptionOp::setup("Unicode: 你好世界 🚀 Ñoño");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set description".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Unicode test".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };

        let yaml = serde_yaml_ng::to_string(&commit).unwrap();
        assert!(yaml.contains("Unicode: 你好世界 🚀 Ñoño"));
        assert!(yaml.contains("author: test-user"));
        assert!(yaml.contains("timestamp: 2024-01-01T00:00:00Z"));
    }

    #[test]
    fn test_roundtrip_single() {
        let op = SetNameOp::setup("Bundle");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set name".to_string(),
        suppress_auto_reindex: false,
        };
        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Setup".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };

        let yaml = serde_yaml_ng::to_string(&commit).unwrap();
        let deserialized: BundleCommit = serde_yaml_ng::from_str(&yaml).unwrap();

        assert_eq!(deserialized.message, "Setup");
        assert_eq!(deserialized.author, "test-user");
        assert_eq!(deserialized.timestamp, "2024-01-01T00:00:00Z");
        assert_eq!(deserialized.operations().len(), 1);
    }

    #[test]
    fn test_roundtrip_complex_operations() {
        // Test that serialization and deserialization are symmetric
        use crate::bundle::operation::{AttachBlockOp, DropColumnOp};
        use crate::data::{BlockId, ObjectId};
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, true),
        ]));

        let attach_config = AttachBlockOp {
            location: "memory:///test".to_string(),
            format: crate::connector::AttachFormat::Parquet,
            version: "v1".to_string(),
            hash: "abcd1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab".to_string(),
            id: BlockId::generate(),
            pack: ObjectId::generate(),
            layout: None,
            num_rows: Some(100),
            bytes: Some(1000),
            schema: "ab/cd1234567890ab.block.schema.yaml".to_string(),
            column_ids: "ab/cd1234567890ab.block.columns.yaml".to_string(),
            schema_cache: Some(schema),
            source_info: None,
            read_options: None,
            column_ids_cache: vec![ColumnId::generate(), ColumnId::generate()],
        };

        let remove_config = DropColumnOp::setup(ColumnId::generate());

        let change = BundleChange {
            id: test_uuid(),
            operations: vec![
                AnyOperation::AttachBlock(attach_config),
                AnyOperation::DropColumn(remove_config),
            ],
            description: "Complex operations".to_string(),
        suppress_auto_reindex: false,
        };

        let commit = BundleCommit {
            url: None,
            data_dir: None,
            message: "Complex ops".to_string(),
            author: "test-user".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            changes: vec![change],
        };

        // Serialize to YAML
        let yaml = serde_yaml_ng::to_string(&commit).unwrap();

        // Verify type field appears for each operation
        assert!(yaml.contains("type: attachBlock"));
        assert!(yaml.contains("type: dropColumn"));

        // Deserialize back
        let deserialized: BundleCommit = serde_yaml_ng::from_str(&yaml).unwrap();

        assert_eq!(deserialized.operations().len(), 2);
        assert!(matches!(
            deserialized.operations()[0],
            AnyOperation::AttachBlock(_)
        ));
        assert!(matches!(
            deserialized.operations()[1],
            AnyOperation::DropColumn(_)
        ));
    }

    #[test]
    fn test_deserialize_operation_with_schema() {
        let yaml = r#"author: test-user
message: Attach data
timestamp: '2024-01-01T00:00:00Z'
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Attach block
  operations:
  - type: attachBlock
    pack: '000000000000003b'
    location: memory:///test_data/userdata.parquet
    format: parquet
    version: test-version
    hash: 0000000000000000000000000000000000000000000000000000000000000000
    id: '000000000000002a'
    numRows: 100
    bytes: 1000
    schema: ab/cd0000000000.block.schema.yaml
    columnIds: ab/cd0000000000.block.columns.yaml
"#;
        let commit: BundleCommit = serde_yaml_ng::from_str(yaml).unwrap();

        assert_eq!(commit.message, "Attach data");
        assert_eq!(commit.operations().len(), 1);

        match &commit.operations()[0] {
            AnyOperation::AttachBlock(config) => {
                assert_eq!(config.location, "memory:///test_data/userdata.parquet");
                assert_eq!(config.version, "test-version");
            }
            _ => panic!("Expected AttachBlock operation"),
        }
    }

    #[test]
    fn test_problem() {
        // Test deserialization with the new structured DataType format
        let yaml = r#"author: nvoxland
message: First commit
timestamp: 2025-11-26T16:20:18Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Attach and transform data
  operations:
  - type: attachBlock
    location: memory:///test_data/userdata.parquet
    format: parquet
    version: '2'
    hash: 0000000000000000000000000000000000000000000000000000000000000000
    id: '00000000000000cc'
    pack: '00000000000000dd'
    numRows: 1000
    bytes: 113629
    schema: ab/cd0000000000.block.schema.yaml
    columnIds: ab/cd0000000000.block.columns.yaml
  - type: dropColumn
    id: '0000000000000aa3'
    name: title
  - type: renameColumn
    id: '0000000000000aa2'
    oldName: first_name
    newName: name
"#;

        let commit: BundleCommit = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(commit.author, "nvoxland");
        assert_eq!(commit.message, "First commit");
        assert_eq!(commit.operations().len(), 3);

        // Verify AttachBlock operation.
        match &commit.operations()[0] {
            AnyOperation::AttachBlock(config) => {
                assert_eq!(config.location, "memory:///test_data/userdata.parquet");
                assert_eq!(config.version, "2");
                assert_eq!(config.schema, "ab/cd0000000000.block.schema.yaml");
                assert!(
                    config.schema_cache.is_none(),
                    "schema_cache is loaded lazily by Bundle::open"
                );
            }
            _ => panic!("Expected AttachBlock operation"),
        }

        // Verify DropColumn operation
        match &commit.operations()[1] {
            AnyOperation::DropColumn(config) => {
                assert_eq!(config.id.to_string(), "0000000000000aa3");
            }
            _ => panic!("Expected DropColumn operation"),
        }

        // Verify RenameColumn operation
        match &commit.operations()[2] {
            AnyOperation::RenameColumn(config) => {
                assert_eq!(config.new_name, "name");
            }
            _ => panic!("Expected RenameColumn operation"),
        }
    }

    #[test]
    fn test_manifest_version_parsing() {
        assert_eq!(manifest_version("00000abc123def456.yaml"), 0);
        assert_eq!(manifest_version("00001a1b2c3d4e5f.yaml"), 1);
        assert_eq!(manifest_version("00042xyz123456789.yaml"), 42);
        assert_eq!(manifest_version("01000abc123def456.yaml"), 1000);
    }
}
