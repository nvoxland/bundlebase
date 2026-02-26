use crate::bundle::operation::AnyOperation;
use crate::data::BlockId;
use crate::object_id::ColumnId;
use arrow::datatypes::{Schema, SchemaRef};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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

/// Check if a column is computed (from AddColumn) vs physical (from AttachBlock).
///
/// Scans operations to determine origin. Returns `true` if the column was
/// created by an AddColumn operation, `false` if it comes from AttachBlock.
pub fn is_computed_column(operations: &[AnyOperation], id: &ColumnId) -> bool {
    for op in operations {
        match op {
            AnyOperation::AddColumn(add) if &add.id == id => return true,
            AnyOperation::AttachBlock(attach) => {
                if attach.column_ids.contains(id) {
                    return false;
                }
            }
            _ => {}
        }
    }
    false
}

/// For a physical column, returns all (block_id, version) pairs from AttachBlock ops
/// that contain the column. For a computed column, returns ALL blocks from ALL
/// AttachBlock ops (since computed columns span all blocks).
pub fn blocks_for_column(operations: &[AnyOperation], id: &ColumnId) -> Vec<(BlockId, String)> {
    let computed = is_computed_column(operations, id);

    let mut blocks = Vec::new();
    for op in operations {
        if let AnyOperation::AttachBlock(attach) = op {
            if computed || attach.column_ids.contains(id) {
                blocks.push((attach.id, attach.version.clone()));
            }
        }
    }
    blocks
}

/// Build a unified physical schema from all AttachBlock operations.
///
/// Iterates AttachBlock ops in order, collecting unique columns by ColumnId.
/// Returns the schema (with fields in first-seen order) and the corresponding
/// column IDs. This represents all physical columns across all blocks.
pub fn unified_physical_schema(operations: &[AnyOperation]) -> (SchemaRef, Vec<ColumnId>) {
    let mut fields: Vec<Arc<arrow::datatypes::Field>> = Vec::new();
    let mut column_ids: Vec<ColumnId> = Vec::new();
    let mut seen_ids: HashSet<ColumnId> = HashSet::new();

    for op in operations {
        if let AnyOperation::AttachBlock(attach) = op {
            if let Some(schema) = &attach.schema {
                for (field, col_id) in schema.fields().iter().zip(attach.column_ids.iter()) {
                    if seen_ids.insert(*col_id) {
                        fields.push(field.clone());
                        column_ids.push(*col_id);
                    }
                }
            }
        }
    }

    (Arc::new(Schema::new(fields)), column_ids)
}

/// For a physical column, returns the original field name from the AttachBlock schema.
///
/// This is the name as it appears in the underlying data file, before any renames.
pub fn physical_column_name(operations: &[AnyOperation], id: &ColumnId) -> Option<String> {
    for op in operations {
        if let AnyOperation::AttachBlock(attach) = op {
            if let Some(pos) = attach.column_ids.iter().position(|cid| cid == id) {
                if let Some(schema) = &attach.schema {
                    return schema.fields().get(pos).map(|f| f.name().to_string());
                }
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

    // --- Test helpers for building fake operations ---

    use crate::bundle::operation::{AttachBlockOp, AddColumnOp, RenameColumnOp, DropColumnOp};
    use crate::data::ObjectId;

    fn fake_attach(names: &[&str], ids: &[ColumnId]) -> AnyOperation {
        let fields: Vec<Arc<Field>> = names
            .iter()
            .map(|n| Arc::new(Field::new(*n, DataType::Utf8, true)))
            .collect();
        AnyOperation::AttachBlock(AttachBlockOp {
            id: BlockId::generate(),
            pack: ObjectId::generate(),
            location: "memory:///fake".to_string(),
            read_options: None,
            version: "v1".to_string(),
            hash: "0".repeat(64),
            source_info: None,
            layout: None,
            num_rows: None,
            bytes: None,
            schema: Some(Arc::new(Schema::new(fields))),
            column_ids: ids.to_vec(),
        })
    }

    fn fake_attach_with_block_id(names: &[&str], ids: &[ColumnId], block_id: BlockId) -> AnyOperation {
        let fields: Vec<Arc<Field>> = names
            .iter()
            .map(|n| Arc::new(Field::new(*n, DataType::Utf8, true)))
            .collect();
        AnyOperation::AttachBlock(AttachBlockOp {
            id: block_id,
            pack: ObjectId::generate(),
            location: "memory:///fake".to_string(),
            read_options: None,
            version: "v1".to_string(),
            hash: "0".repeat(64),
            source_info: None,
            layout: None,
            num_rows: None,
            bytes: None,
            schema: Some(Arc::new(Schema::new(fields))),
            column_ids: ids.to_vec(),
        })
    }

    fn fake_add_column(id: ColumnId, name: &str) -> AnyOperation {
        AnyOperation::AddColumn(AddColumnOp {
            id,
            name: name.to_string(),
            expression: "1 + 1".to_string(),
        })
    }

    fn fake_rename(id: ColumnId, new_name: &str) -> AnyOperation {
        AnyOperation::RenameColumn(RenameColumnOp {
            id,
            new_name: new_name.to_string(),
        })
    }

    fn fake_drop(id: ColumnId) -> AnyOperation {
        AnyOperation::DropColumn(DropColumnOp { id })
    }

    // --- initial_column_names tests ---

    #[test]
    fn test_initial_column_names_from_attach() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let ops = vec![fake_attach(&["col_a", "col_b"], &[id_a, id_b])];

        let names = initial_column_names(&ops);
        assert_eq!(names.len(), 2);
        assert_eq!(names.get(&id_a), Some(&"col_a".to_string()));
        assert_eq!(names.get(&id_b), Some(&"col_b".to_string()));
    }

    #[test]
    fn test_initial_column_names_deduplicates_across_blocks() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a", "col_b"], &[id_a, id_b]),
            fake_attach(&["col_a", "col_b"], &[id_a, id_b]),
        ];

        let names = initial_column_names(&ops);
        assert_eq!(names.len(), 2, "Should deduplicate shared ColumnIds across blocks");
    }

    #[test]
    fn test_initial_column_names_includes_add_column() {
        let id_a = ColumnId::generate();
        let id_computed = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a"], &[id_a]),
            fake_add_column(id_computed, "computed"),
        ];

        let names = initial_column_names(&ops);
        assert_eq!(names.len(), 2);
        assert_eq!(names.get(&id_computed), Some(&"computed".to_string()));
    }

    #[test]
    fn test_initial_column_names_ignores_renames_and_drops() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a", "col_b"], &[id_a, id_b]),
            fake_rename(id_a, "renamed_a"),
            fake_drop(id_b),
        ];

        let names = initial_column_names(&ops);
        assert_eq!(names.len(), 2, "Renames/drops should not affect initial names");
        assert_eq!(names.get(&id_a), Some(&"col_a".to_string()), "Should keep original name");
        assert_eq!(names.get(&id_b), Some(&"col_b".to_string()), "Dropped column should still appear");
    }

    // --- resolved_column_names tests ---

    #[test]
    fn test_resolved_column_names_applies_rename_and_drop() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a", "col_b"], &[id_a, id_b]),
            fake_rename(id_a, "renamed_a"),
            fake_drop(id_b),
        ];

        let names = resolved_column_names(&ops);
        assert_eq!(names.len(), 1, "Dropped column should be removed");
        assert_eq!(names.get(&id_a), Some(&"renamed_a".to_string()));
        assert!(names.get(&id_b).is_none(), "Dropped column should not appear");
    }

    // --- unified_physical_schema tests ---

    #[test]
    fn test_unified_physical_schema_deduplicates_by_id() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let id_c = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a", "col_b"], &[id_a, id_b]),
            fake_attach(&["col_a", "col_c"], &[id_a, id_c]),
        ];

        let (schema, col_ids) = unified_physical_schema(&ops);
        assert_eq!(schema.fields().len(), 3, "Should have 3 unique fields: a, b, c");
        assert_eq!(col_ids.len(), 3);
        assert_eq!(schema.field(0).name(), "col_a");
        assert_eq!(schema.field(1).name(), "col_b");
        assert_eq!(schema.field(2).name(), "col_c");
    }

    // --- is_computed_column tests ---

    #[test]
    fn test_is_computed_column() {
        let id_physical = ColumnId::generate();
        let id_computed = ColumnId::generate();
        let id_unknown = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a"], &[id_physical]),
            fake_add_column(id_computed, "computed"),
        ];

        assert!(!is_computed_column(&ops, &id_physical), "AttachBlock column is not computed");
        assert!(is_computed_column(&ops, &id_computed), "AddColumn column is computed");
        assert!(!is_computed_column(&ops, &id_unknown), "Unknown column is not computed");
    }

    // --- blocks_for_column tests ---

    #[test]
    fn test_blocks_for_physical_column() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let block1 = BlockId::generate();
        let block2 = BlockId::generate();
        let ops = vec![
            fake_attach_with_block_id(&["col_a", "col_b"], &[id_a, id_b], block1),
            fake_attach_with_block_id(&["col_a"], &[id_a], block2),
        ];

        let blocks = blocks_for_column(&ops, &id_a);
        assert_eq!(blocks.len(), 2, "col_a appears in both blocks");

        let blocks_b = blocks_for_column(&ops, &id_b);
        assert_eq!(blocks_b.len(), 1, "col_b appears in only block1");
        assert_eq!(blocks_b[0].0, block1);
    }

    #[test]
    fn test_blocks_for_computed_column() {
        let id_a = ColumnId::generate();
        let id_computed = ColumnId::generate();
        let block1 = BlockId::generate();
        let block2 = BlockId::generate();
        let ops = vec![
            fake_attach_with_block_id(&["col_a"], &[id_a], block1),
            fake_attach_with_block_id(&["col_a"], &[id_a], block2),
            fake_add_column(id_computed, "computed"),
        ];

        let blocks = blocks_for_column(&ops, &id_computed);
        assert_eq!(blocks.len(), 2, "Computed column spans ALL blocks");
    }

    // --- physical_column_name tests ---

    #[test]
    fn test_physical_column_name() {
        let id_a = ColumnId::generate();
        let id_computed = ColumnId::generate();
        let id_unknown = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a"], &[id_a]),
            fake_add_column(id_computed, "computed"),
        ];

        assert_eq!(physical_column_name(&ops, &id_a), Some("col_a".to_string()));
        assert_eq!(physical_column_name(&ops, &id_computed), None, "Computed column has no physical name");
        assert_eq!(physical_column_name(&ops, &id_unknown), None, "Unknown column has no physical name");
    }
}
