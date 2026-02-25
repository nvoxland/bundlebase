use crate::bundle::operation::AnyOperation;
use crate::object_id::ColumnId;
use arrow::datatypes::Schema;
use std::collections::HashMap;

pub const COLUMN_ID_KEY: &str = "bundlebase:column_id";
pub const ORIGINAL_NAME_KEY: &str = "bundlebase:original_name";

/// Lightweight map of column ID → current name, threaded through apply_dataframe calls.
pub type ColumnNames = HashMap<ColumnId, String>;

/// Build initial ColumnNames from an operation list.
///
/// Only populates from AttachBlock and AddColumn operations. Renames and drops
/// are NOT applied here — those mutations happen incrementally as each operation's
/// `apply_dataframe` updates the map during the apply loop.
pub fn initial_column_names(operations: &[AnyOperation]) -> ColumnNames {
    let mut id_to_name: ColumnNames = HashMap::new();
    let mut name_to_id: HashMap<String, ColumnId> = HashMap::new();

    for op in operations {
        match op {
            AnyOperation::AttachBlock(attach) => {
                if let Some(schema) = &attach.schema {
                    for (field, id) in schema.fields().iter().zip(attach.column_ids.iter()) {
                        let fname = field.name().to_string();
                        if !name_to_id.contains_key(&fname) {
                            name_to_id.insert(fname.clone(), *id);
                            id_to_name.insert(*id, fname);
                        }
                    }
                }
            }
            AnyOperation::AddColumn(add) => {
                if !name_to_id.contains_key(&add.name) {
                    name_to_id.insert(add.name.clone(), add.id);
                    id_to_name.insert(add.id, add.name.clone());
                }
            }
            _ => {}
        }
    }

    id_to_name
}

/// Build a fully-resolved ColumnNames map from an operation list.
///
/// Unlike `initial_column_names`, this applies all mutations (renames, drops) to produce
/// the final state after all operations. Used by `check()` methods that need to know
/// the current column names after all operations have been applied.
pub fn resolved_column_names(operations: &[AnyOperation]) -> ColumnNames {
    let mut id_to_name: ColumnNames = HashMap::new();
    let mut name_to_id: HashMap<String, ColumnId> = HashMap::new();

    for op in operations {
        match op {
            AnyOperation::AttachBlock(attach) => {
                if let Some(schema) = &attach.schema {
                    for (field, id) in schema.fields().iter().zip(attach.column_ids.iter()) {
                        let fname = field.name().to_string();
                        if !name_to_id.contains_key(&fname) {
                            name_to_id.insert(fname.clone(), *id);
                            id_to_name.insert(*id, fname);
                        }
                    }
                }
            }
            AnyOperation::AddColumn(add) => {
                if !name_to_id.contains_key(&add.name) {
                    name_to_id.insert(add.name.clone(), add.id);
                    id_to_name.insert(add.id, add.name.clone());
                }
            }
            AnyOperation::RenameColumn(rename) => {
                if let Some(old_name) = id_to_name.get(&rename.id).cloned() {
                    name_to_id.remove(&old_name);
                }
                name_to_id.insert(rename.new_name.clone(), rename.id);
                id_to_name.insert(rename.id, rename.new_name.clone());
            }
            AnyOperation::DropColumn(drop_op) => {
                if let Some(name) = id_to_name.remove(&drop_op.id) {
                    name_to_id.remove(&name);
                }
            }
            _ => {}
        }
    }

    id_to_name
}

/// Find a column's current name by its ColumnId in the schema metadata.
pub fn column_name_by_id(schema: &Schema, id: &ColumnId) -> Option<String> {
    let id_str = id.to_string();
    for field in schema.fields() {
        if let Some(meta_id) = field.metadata().get(COLUMN_ID_KEY) {
            if meta_id == &id_str {
                return Some(field.name().clone());
            }
        }
    }
    None
}

/// Find a column's ID by its current name in the schema metadata.
pub fn column_id_for_name(schema: &Schema, name: &str) -> Option<ColumnId> {
    schema
        .column_with_name(name)
        .and_then(|(_, field)| {
            field
                .metadata()
                .get(COLUMN_ID_KEY)
                .and_then(|s| ColumnId::try_from(s.as_str()).ok())
        })
}

/// Find a column's original (physical) name by its ColumnId.
pub fn original_name_by_id(schema: &Schema, id: &ColumnId) -> Option<String> {
    let id_str = id.to_string();
    for field in schema.fields() {
        if let Some(meta_id) = field.metadata().get(COLUMN_ID_KEY) {
            if meta_id == &id_str {
                return field.metadata().get(ORIGINAL_NAME_KEY).cloned();
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field};

    fn schema_with_metadata() -> Schema {
        let id1 = ColumnId::generate();
        let id2 = ColumnId::generate();

        let mut meta1 = HashMap::new();
        meta1.insert(COLUMN_ID_KEY.to_string(), id1.to_string());
        meta1.insert(ORIGINAL_NAME_KEY.to_string(), "col_a".to_string());

        let mut meta2 = HashMap::new();
        meta2.insert(COLUMN_ID_KEY.to_string(), id2.to_string());
        meta2.insert(ORIGINAL_NAME_KEY.to_string(), "col_b".to_string());

        Schema::new(vec![
            Field::new("renamed_a", DataType::Utf8, true).with_metadata(meta1),
            Field::new("col_b", DataType::Int64, true).with_metadata(meta2),
        ])
    }

    #[test]
    fn test_column_name_by_id() {
        let schema = schema_with_metadata();
        let id_str = schema.field(0).metadata().get(COLUMN_ID_KEY).cloned().expect("has id");
        let id = ColumnId::try_from(id_str.as_str()).expect("valid id");

        assert_eq!(column_name_by_id(&schema, &id), Some("renamed_a".to_string()));
    }

    #[test]
    fn test_column_id_for_name() {
        let schema = schema_with_metadata();
        let expected_id_str = schema.field(1).metadata().get(COLUMN_ID_KEY).cloned().expect("has id");
        let expected_id = ColumnId::try_from(expected_id_str.as_str()).expect("valid id");

        assert_eq!(column_id_for_name(&schema, "col_b"), Some(expected_id));
        assert_eq!(column_id_for_name(&schema, "nonexistent"), None);
    }

    #[test]
    fn test_original_name_by_id() {
        let schema = schema_with_metadata();
        let id_str = schema.field(0).metadata().get(COLUMN_ID_KEY).cloned().expect("has id");
        let id = ColumnId::try_from(id_str.as_str()).expect("valid id");

        assert_eq!(original_name_by_id(&schema, &id), Some("col_a".to_string()));
    }

    #[test]
    fn test_column_name_by_id_not_found() {
        let schema = schema_with_metadata();
        let unknown_id = ColumnId::generate();
        assert_eq!(column_name_by_id(&schema, &unknown_id), None);
    }
}
