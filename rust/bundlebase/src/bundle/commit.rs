use arrow::array::{ArrayRef, Int32Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use std::sync::Arc;

use crate::bundle::command::response::{CommandResponse, OutputShape};
use crate::impl_dyn_command_response;
use crate::bundle::operation::{AnyOperation, BundleChange};
use crate::BundlebaseError;
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

/// CommandResponse implementation for displaying commit history.
impl CommandResponse for Vec<BundleCommit> {
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

    fn to_record_batch(&self) -> Result<RecordBatch, BundlebaseError> {
        let ids: Vec<i32> = (0..self.len() as i32).collect();
        let urls: Vec<Option<String>> = self
            .iter()
            .map(|c| c.url.as_ref().map(|u| u.to_string()))
            .collect();
        let authors: Vec<&str> = self.iter().map(|c| c.author.as_str()).collect();
        let messages: Vec<&str> = self.iter().map(|c| c.message.as_str()).collect();
        let timestamps: Vec<&str> = self.iter().map(|c| c.timestamp.as_str()).collect();
        let change_counts: Vec<i32> = self.iter().map(|c| c.changes.len() as i32).collect();

        RecordBatch::try_new(
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
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))
    }

    impl_dyn_command_response!(Vec<BundleCommit>);
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
        let op = DropColumnOp::setup(vec!["col1"]);
        let change = BundleChange {
            id: test_uuid(),
            description: "Remove columns".to_string(),
            operations: vec![op.into()],
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

        let expected = r"author: test-user
message: Remove column
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Remove columns
  operations:
  - type: dropColumn
    names:
    - col1
";
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_multiple_operations() {
        let op1 = SetNameOp::setup("Test");
        let op2 = DropColumnOp::setup(vec!["col1"]);
        let op3 = RenameColumnOp::setup("old", "new");

        let change = BundleChange {
            id: test_uuid(),
            description: "Multiple operations".to_string(),
            operations: vec![op1.into(), op2.into(), op3.into()],
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

        let expected = r"author: test-user
message: Multiple ops
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Multiple operations
  operations:
  - type: setName
    name: Test
  - type: dropColumn
    names:
    - col1
  - type: renameColumn
    oldName: old
    newName: new
";
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_with_from() {
        let op = SetNameOp::setup("Test");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set name".to_string(),
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
    names:
    - col1
  - type: renameColumn
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
        let op = RenameColumnOp::setup("firstName", "first_name");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Rename column".to_string(),
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

        // Should have oldName and newName in camelCase
        let expected = r"author: test-user
message: Test camelCase
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Rename column
  operations:
  - type: renameColumn
    oldName: firstName
    newName: first_name
";
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_type_always_first() {
        // Verify that "type" field is always added first in the mapping
        let op = RenameColumnOp::setup("a", "b");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Rename".to_string(),
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
        let expected = r"author: test-user
message: Test
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Rename
  operations:
  - type: renameColumn
    oldName: a
    newName: b
";
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_serialize_set_name() {
        let op = SetNameOp::setup("My Bundle");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set name".to_string(),
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
        let op = RenameColumnOp::setup("col_with_underscore", "col-with-dash");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Rename".to_string(),
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
        let expected = r"author: test-user
message: Rename
timestamp: 2024-01-01T00:00:00Z
changes:
- id: 12345678-1234-1234-1234-123456789012
  description: Rename
  operations:
  - type: renameColumn
    oldName: col_with_underscore
    newName: col-with-dash
";
        assert_eq!(yaml, expected);
    }

    #[test]
    fn test_empty_string_values() {
        let op = SetNameOp::setup("");
        let change = BundleChange {
            id: test_uuid(),
            operations: vec![op.into()],
            description: "Set name".to_string(),
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
        use crate::data::ObjectId;
        use arrow_schema::{DataType, Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, true),
        ]));

        let attach_config = AttachBlockOp {
            location: "memory:///test".to_string(),
            version: "v1".to_string(),
            hash: "abcd1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab".to_string(),
            id: ObjectId::from(42u8),
            pack: ObjectId::from(53u8),
            layout: None,
            num_rows: Some(100),
            bytes: Some(1000),
            schema: Some(schema),
            source_info: None,
        };

        let remove_config = DropColumnOp {
            names: vec!["col1".to_string()],
        };

        let change = BundleChange {
            id: test_uuid(),
            operations: vec![
                AnyOperation::AttachBlock(attach_config),
                AnyOperation::DropColumn(remove_config),
            ],
            description: "Complex operations".to_string(),
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
    pack: '3b'
    location: memory:///test_data/userdata.parquet
    version: test-version
    hash: 0000000000000000000000000000000000000000000000000000000000000000
    id: '2a'
    numRows: 100
    bytes: 1000
    schema:
      fields:
      - name: id
        data_type: Int32
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {}
      metadata: {}
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
    version: '2'
    hash: 0000000000000000000000000000000000000000000000000000000000000000
    id: cc
    pack: dd
    numRows: 1000
    bytes: 113629
    schema:
      fields:
      - name: registration_dttm
        data_type:
          type: Timestamp
          unit: Nanosecond
          timezone: null
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {}
      - name: id
        data_type: Int32
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {}
      - name: first_name
        data_type: Utf8View
        nullable: true
        dict_id: 0
        dict_is_ordered: false
        metadata: {}
      metadata: {}
  - type: dropColumn
    names:
    - title
  - type: renameColumn
    oldName: first_name
    newName: name
"#;

        let commit: BundleCommit = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(commit.author, "nvoxland");
        assert_eq!(commit.message, "First commit");
        assert_eq!(commit.operations().len(), 3);

        // Verify AttachBlock operation
        match &commit.operations()[0] {
            AnyOperation::AttachBlock(config) => {
                assert_eq!(config.location, "memory:///test_data/userdata.parquet");
                assert_eq!(config.version, "2");
                assert!(config.schema.is_some());
                let schema = config.schema.as_ref().unwrap();
                assert_eq!(schema.fields().len(), 3);
                // Verify first field has Timestamp data type
                assert_eq!(schema.field(0).name(), "registration_dttm");
            }
            _ => panic!("Expected AttachBlock operation"),
        }

        // Verify RemoveColumns operation
        match &commit.operations()[1] {
            AnyOperation::DropColumn(config) => {
                assert_eq!(config.names, vec!["title".to_string()]);
            }
            _ => panic!("Expected RemoveColumns operation"),
        }

        // Verify RenameColumn operation
        match &commit.operations()[2] {
            AnyOperation::RenameColumn(config) => {
                assert_eq!(config.old_name, "first_name");
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
