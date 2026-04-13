use crate::bundle::operation::AnyOperation;
use crate::data::BlockId;
use crate::object_id::ColumnId;
use arrow::datatypes::{Schema, SchemaRef};
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;


/// Prefix used for stable internal column names (`col_<hex_id>`).
const INTERNAL_NAME_PREFIX: &str = "col_";

/// Column registry that tracks the mapping between ColumnIds, user-visible names,
/// internal names, and cached Arrow schemas.
///
/// Threaded through `apply_dataframe` calls so operations can update column names
/// as they apply renames, adds, and drops.
#[derive(Debug, Clone)]
pub struct BundleSchema {
    columns: HashMap<ColumnId, String>,
    computed_columns: HashSet<ColumnId>,
    /// All (block_id, version) pairs from AttachBlock ops, used to resolve
    /// which blocks contain a given column (or all blocks for computed columns).
    all_blocks: Vec<(BlockId, String)>,
    /// Maps each physical ColumnId to the blocks that contain it.
    column_blocks: HashMap<ColumnId, Vec<(BlockId, String)>>,
    /// Unified physical schema: all unique physical columns across all blocks,
    /// deduplicated by ColumnId, with internal names. Built from AttachBlock ops.
    physical_schema: Option<SchemaRef>,
    /// Ordered ColumnIds corresponding to the physical_schema fields.
    physical_column_ids: Vec<ColumnId>,
    schema: Option<SchemaRef>,
    internal_schema: Option<SchemaRef>,
}

impl BundleSchema {
    /// Create an empty BundleSchema.
    pub fn new() -> Self {
        Self {
            columns: HashMap::new(),
            computed_columns: HashSet::new(),
            all_blocks: Vec::new(),
            column_blocks: HashMap::new(),
            physical_schema: None,
            physical_column_ids: Vec::new(),
            schema: None,
            internal_schema: None,
        }
    }

    /// Build initial column names from an operation list.
    ///
    /// Only populates from AttachBlock and AddColumn operations. Renames and drops
    /// are NOT applied here — those mutations happen incrementally as each operation's
    /// `apply_dataframe` updates the registry during the apply loop.
    pub fn initial(operations: &[AnyOperation]) -> Self {
        let mut id_to_name: HashMap<ColumnId, String> = HashMap::new();
        let mut name_to_id: HashMap<String, ColumnId> = HashMap::new();
        let mut computed: HashSet<ColumnId> = HashSet::new();
        let mut all_blocks: Vec<(BlockId, String)> = Vec::new();
        let mut col_blocks: HashMap<ColumnId, Vec<(BlockId, String)>> = HashMap::new();
        let mut physical_fields: Vec<Arc<arrow::datatypes::Field>> = Vec::new();
        let mut physical_col_ids: Vec<ColumnId> = Vec::new();
        let mut seen_physical: HashSet<ColumnId> = HashSet::new();

        for op in operations {
            match op {
                AnyOperation::CreateSource(src) => {
                    // Pre-register column IDs from expected_schema so column ops
                    // (RENAME, CAST, etc.) can reference them before any data is fetched.
                    if let Some(ref expected) = src.expected_schema {
                        for col in expected {
                            if !name_to_id.contains_key(&col.name) {
                                name_to_id.insert(col.name.clone(), col.id);
                                id_to_name.insert(col.id, col.name.clone());
                            }
                        }
                    }
                }
                AnyOperation::AttachBlock(attach) => {
                    let block_entry = (attach.id, attach.version.clone());
                    all_blocks.push(block_entry.clone());
                    if let Some(schema) = &attach.schema {
                        for (field, id) in schema.fields().iter().zip(attach.column_ids.iter()) {
                            col_blocks.entry(*id).or_default().push(block_entry.clone());
                            if seen_physical.insert(*id) {
                                physical_fields.push(Arc::new(
                                    field.as_ref().clone().with_name(generate_internal_name(id))
                                ));
                                physical_col_ids.push(*id);
                            }
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
                        computed.insert(add.id);
                    }
                }
                _ => {}
            }
        }

        Self {
            columns: id_to_name,
            computed_columns: computed,
            all_blocks,
            column_blocks: col_blocks,
            physical_schema: Some(Arc::new(Schema::new(physical_fields))),
            physical_column_ids: physical_col_ids,
            schema: None,
            internal_schema: None,
        }
    }

    /// Build a fully-resolved column registry from an operation list.
    ///
    /// Unlike `initial`, this applies all mutations (renames, drops) to produce
    /// the final state after all operations. Used by `check()` methods that need to know
    /// the current column names after all operations have been applied.
    pub fn resolved(operations: &[AnyOperation]) -> Self {
        let mut id_to_name: HashMap<ColumnId, String> = HashMap::new();
        let mut name_to_id: HashMap<String, ColumnId> = HashMap::new();
        let mut computed: HashSet<ColumnId> = HashSet::new();
        let mut all_blocks: Vec<(BlockId, String)> = Vec::new();
        let mut col_blocks: HashMap<ColumnId, Vec<(BlockId, String)>> = HashMap::new();
        let mut physical_fields: Vec<Arc<arrow::datatypes::Field>> = Vec::new();
        let mut physical_col_ids: Vec<ColumnId> = Vec::new();
        let mut seen_physical: HashSet<ColumnId> = HashSet::new();

        for op in operations {
            match op {
                AnyOperation::CreateSource(src) => {
                    // Pre-register column IDs from expected_schema.
                    if let Some(ref expected) = src.expected_schema {
                        for col in expected {
                            if !name_to_id.contains_key(&col.name) {
                                name_to_id.insert(col.name.clone(), col.id);
                                id_to_name.insert(col.id, col.name.clone());
                            }
                        }
                    }
                }
                AnyOperation::AttachBlock(attach) => {
                    let block_entry = (attach.id, attach.version.clone());
                    all_blocks.push(block_entry.clone());
                    if let Some(schema) = &attach.schema {
                        for (field, id) in schema.fields().iter().zip(attach.column_ids.iter()) {
                            col_blocks.entry(*id).or_default().push(block_entry.clone());
                            if seen_physical.insert(*id) {
                                physical_fields.push(Arc::new(
                                    field.as_ref().clone().with_name(generate_internal_name(id))
                                ));
                                physical_col_ids.push(*id);
                            }
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
                        computed.insert(add.id);
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
                    computed.remove(&drop_op.id);
                }
                _ => {}
            }
        }

        Self {
            columns: id_to_name,
            computed_columns: computed,
            all_blocks,
            column_blocks: col_blocks,
            physical_schema: Some(Arc::new(Schema::new(physical_fields))),
            physical_column_ids: physical_col_ids,
            schema: None,
            internal_schema: None,
        }
    }

    // --- Column lookup ---

    /// Resolve a user-visible column name to its ColumnId.
    pub fn column_id(&self, name: &str) -> Option<ColumnId> {
        self.columns.iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(id, _)| *id)
    }

    /// Resolve a ColumnId to its current user-visible name.
    pub fn column_name(&self, id: &ColumnId) -> Option<String> {
        self.columns.get(id).cloned()
    }

    /// Returns the stable internal name (`col_<hex>`) for a known column.
    /// Returns an error if the ColumnId is not registered in this schema.
    pub fn internal_name(&self, id: &ColumnId) -> Result<String, crate::BundlebaseError> {
        if self.columns.contains_key(id) || self.computed_columns.contains(id) {
            Ok(generate_internal_name(id))
        } else {
            Err(format!("Column ID '{}' not found in schema", id).into())
        }
    }

    /// Returns an unqualified DataFusion Column for a known column's internal name.
    /// Returns an error if the ColumnId is not registered in this schema.
    pub fn internal_column(&self, id: &ColumnId) -> Result<datafusion::common::Column, crate::BundlebaseError> {
        Ok(datafusion::common::Column::new_unqualified(self.internal_name(id)?))
    }

    /// Check if a column is computed (from AddColumn) vs physical (from AttachBlock).
    pub fn is_computed(&self, id: &ColumnId) -> bool {
        self.computed_columns.contains(id)
    }

    /// Returns the (block_id, version) pairs that contain a given column.
    /// For computed columns, returns ALL blocks (since computed columns span all blocks).
    pub fn blocks_for_column(&self, id: &ColumnId) -> Vec<(BlockId, String)> {
        if self.computed_columns.contains(id) {
            self.all_blocks.clone()
        } else {
            self.column_blocks.get(id).cloned().unwrap_or_default()
        }
    }

    /// Returns the unified physical schema: all unique physical columns across all blocks,
    /// deduplicated by ColumnId, with internal names and original field types.
    pub fn physical_schema(&self) -> SchemaRef {
        self.physical_schema.clone().unwrap_or_else(|| Arc::new(Schema::empty()))
    }

    /// Returns the ordered ColumnIds corresponding to the physical schema fields.
    pub fn physical_column_ids(&self) -> &[ColumnId] {
        &self.physical_column_ids
    }

    /// Returns the raw column ID → name map.
    pub fn columns(&self) -> &HashMap<ColumnId, String> {
        &self.columns
    }

    // --- Column mutation ---

    /// Register or update a column's user-visible name.
    pub fn insert(&mut self, id: ColumnId, name: String) {
        self.columns.insert(id, name);
    }

    /// Register a computed column (from AddColumn).
    pub fn insert_computed(&mut self, id: ColumnId, name: String) {
        self.columns.insert(id, name);
        self.computed_columns.insert(id);
    }

    /// Remove a column from the registry. Returns the old name if present.
    pub fn remove(&mut self, id: &ColumnId) -> Option<String> {
        self.columns.remove(id)
    }

    /// Access a mutable entry for a ColumnId.
    pub fn entry(&mut self, id: ColumnId) -> std::collections::hash_map::Entry<'_, ColumnId, String> {
        self.columns.entry(id)
    }

    // --- Schema management ---

    /// Returns the cached user-visible schema, if set.
    pub fn schema(&self) -> Option<&SchemaRef> {
        self.schema.as_ref()
    }

    /// Returns the internal-name schema, computing it lazily from the user-visible schema.
    pub fn internal_schema(&self) -> Option<SchemaRef> {
        let schema = self.schema.as_ref()?;
        if let Some(cached) = &self.internal_schema {
            return Some(cached.clone());
        }
        let internal_fields: Vec<Arc<arrow::datatypes::Field>> = schema.fields().iter().filter_map(|f| {
            self.columns.iter()
                .find(|(_, name)| name.as_str() == f.name())
                .map(|(id, _)| {
                    Arc::new(f.as_ref().clone().with_name(generate_internal_name(id)))
                })
        }).collect();
        Some(Arc::new(Schema::new(internal_fields)))
    }

    /// Set the user-visible schema. The internal-name schema will be computed lazily.
    pub fn set_schema(&mut self, schema: SchemaRef) {
        self.schema = Some(schema);
        self.internal_schema = None;
    }

    // --- SQL translation ---

    /// Build a reverse map of user-visible name → internal name.
    fn name_to_internal_name_map(&self) -> HashMap<String, String> {
        self.columns
            .iter()
            .map(|(id, user_name)| (user_name.clone(), generate_internal_name(id)))
            .collect()
    }

    /// Translate user-visible column names in a SQL fragment to stable internal names.
    ///
    /// Uses word-boundary matching: only replaces identifiers that appear as whole words
    /// (not inside other identifiers). Handles both bare and double-quoted identifiers.
    /// Longer names are replaced first to avoid partial matches.
    pub fn translate_sql(&self, sql: &str) -> String {
        let name_map = self.name_to_internal_name_map();

        // Sort by name length descending to avoid partial matches (e.g., "id" inside "identity")
        let mut names: Vec<(&String, &String)> = name_map.iter().collect();
        names.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        let mut result = sql.to_string();
        for (user_name, internal_col_name) in names {
            // Replace double-quoted identifiers: "name" → internal name
            let quoted = format!("\"{}\"", user_name);
            let quoted_replacement = format!("\"{}\"", internal_col_name);
            result = result.replace(&quoted, &quoted_replacement);

            // Replace bare identifiers using word-boundary matching
            // A word boundary is: start of string, non-alphanumeric/underscore character
            let mut new_result = String::with_capacity(result.len());
            let name_bytes = user_name.as_bytes();
            let result_bytes = result.as_bytes();
            let mut i = 0;
            while i < result_bytes.len() {
                if i + name_bytes.len() <= result_bytes.len()
                    && &result_bytes[i..i + name_bytes.len()] == name_bytes
                {
                    // Check left boundary: start of string or non-identifier char
                    let left_ok = i == 0 || !is_ident_char(result_bytes[i - 1]);
                    // Check right boundary: end of string or non-identifier char
                    let right_ok = i + name_bytes.len() == result_bytes.len()
                        || !is_ident_char(result_bytes[i + name_bytes.len()]);

                    if left_ok && right_ok {
                        new_result.push_str(internal_col_name);
                        i += name_bytes.len();
                        continue;
                    }
                }
                new_result.push(result_bytes[i] as char);
                i += 1;
            }
            result = new_result;
        }

        result
    }

    /// Rename DataFrame columns from internal names to user-visible names.
    pub fn rename_to_real_names(&self, mut df: datafusion::dataframe::DataFrame) -> Result<datafusion::dataframe::DataFrame, crate::BundlebaseError> {
        for (id, user_name) in &self.columns {
            let int_name = generate_internal_name(id);
            if df.schema().has_column_with_unqualified_name(&int_name) {
                df = df.with_column_renamed(&int_name, user_name)
                    .map_err(|e| Box::new(e) as crate::BundlebaseError)?;
            }
        }
        Ok(df)
    }
}

impl Deref for BundleSchema {
    type Target = HashMap<ColumnId, String>;

    fn deref(&self) -> &Self::Target {
        &self.columns
    }
}

/// Return the stable internal column name for a ColumnId: `col_<hex_id>`.
pub fn generate_internal_name(id: &ColumnId) -> String {
    format!("{}{}", INTERNAL_NAME_PREFIX, id)
}

/// Parse a `col_<hex_id>` string back into a ColumnId.
/// Returns `None` if the string doesn't match the expected format.
pub fn parse_internal_name(name: &str) -> Option<ColumnId> {
    name.strip_prefix(INTERNAL_NAME_PREFIX)
        .and_then(|hex| ColumnId::try_from(hex).ok())
}






fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field};

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
            format: crate::connector::AttachFormat::Parquet,
            read_options: None,
            version: "v1".to_string(),
            hash: "0".repeat(64),
            source_info: None,
            layout: None,
            num_rows: None,
            bytes: None,
            schema_path: "00/00000000000000.block.schema.yaml".to_string(),
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
            format: crate::connector::AttachFormat::Parquet,
            read_options: None,
            version: "v1".to_string(),
            hash: "0".repeat(64),
            source_info: None,
            layout: None,
            num_rows: None,
            bytes: None,
            schema_path: "00/00000000000000.block.schema.yaml".to_string(),
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

    // --- BundleSchema::initial tests ---

    #[test]
    fn test_initial_column_names_from_attach() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let ops = vec![fake_attach(&["col_a", "col_b"], &[id_a, id_b])];

        let schema = BundleSchema::initial(&ops);
        assert_eq!(schema.len(), 2);
        assert_eq!(schema.get(&id_a), Some(&"col_a".to_string()));
        assert_eq!(schema.get(&id_b), Some(&"col_b".to_string()));
    }

    #[test]
    fn test_initial_column_names_deduplicates_across_blocks() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a", "col_b"], &[id_a, id_b]),
            fake_attach(&["col_a", "col_b"], &[id_a, id_b]),
        ];

        let schema = BundleSchema::initial(&ops);
        assert_eq!(schema.len(), 2, "Should deduplicate shared ColumnIds across blocks");
    }

    #[test]
    fn test_initial_column_names_includes_add_column() {
        let id_a = ColumnId::generate();
        let id_computed = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a"], &[id_a]),
            fake_add_column(id_computed, "computed"),
        ];

        let schema = BundleSchema::initial(&ops);
        assert_eq!(schema.len(), 2);
        assert_eq!(schema.get(&id_computed), Some(&"computed".to_string()));
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

        let schema = BundleSchema::initial(&ops);
        assert_eq!(schema.len(), 2, "Renames/drops should not affect initial names");
        assert_eq!(schema.get(&id_a), Some(&"col_a".to_string()), "Should keep original name");
        assert_eq!(schema.get(&id_b), Some(&"col_b".to_string()), "Dropped column should still appear");
    }

    // --- BundleSchema::resolved tests ---

    #[test]
    fn test_resolved_column_names_applies_rename_and_drop() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a", "col_b"], &[id_a, id_b]),
            fake_rename(id_a, "renamed_a"),
            fake_drop(id_b),
        ];

        let schema = BundleSchema::resolved(&ops);
        assert_eq!(schema.len(), 1, "Dropped column should be removed");
        assert_eq!(schema.get(&id_a), Some(&"renamed_a".to_string()));
        assert!(schema.get(&id_b).is_none(), "Dropped column should not appear");
    }

    // --- BundleSchema::column_id / column_name tests ---

    #[test]
    fn test_bundle_schema_column_id_lookup() {
        let id_a = ColumnId::generate();
        let ops = vec![fake_attach(&["col_a"], &[id_a])];
        let schema = BundleSchema::resolved(&ops);

        assert_eq!(schema.column_id("col_a"), Some(id_a));
        assert_eq!(schema.column_id("nonexistent"), None);
    }

    #[test]
    fn test_bundle_schema_column_name_lookup() {
        let id_a = ColumnId::generate();
        let ops = vec![fake_attach(&["col_a"], &[id_a])];
        let schema = BundleSchema::resolved(&ops);

        assert_eq!(schema.column_name(&id_a), Some("col_a".to_string()));
        assert_eq!(schema.column_name(&ColumnId::generate()), None);
    }

    // --- BundleSchema::physical_schema tests ---

    #[test]
    fn test_physical_schema_deduplicates_by_id() {
        let id_a = ColumnId::generate();
        let id_b = ColumnId::generate();
        let id_c = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a", "col_b"], &[id_a, id_b]),
            fake_attach(&["col_a", "col_c"], &[id_a, id_c]),
        ];

        let bs = BundleSchema::initial(&ops);
        let schema = bs.physical_schema();
        let col_ids = bs.physical_column_ids();
        assert_eq!(schema.fields().len(), 3, "Should have 3 unique fields: a, b, c");
        assert_eq!(col_ids.len(), 3);
        // Fields use internal names instead of physical names
        assert_eq!(schema.field(0).name(), &generate_internal_name(&id_a));
        assert_eq!(schema.field(1).name(), &generate_internal_name(&id_b));
        assert_eq!(schema.field(2).name(), &generate_internal_name(&id_c));
    }

    // --- BundleSchema::is_computed tests ---

    #[test]
    fn test_is_computed_column() {
        let id_physical = ColumnId::generate();
        let id_computed = ColumnId::generate();
        let id_unknown = ColumnId::generate();
        let ops = vec![
            fake_attach(&["col_a"], &[id_physical]),
            fake_add_column(id_computed, "computed"),
        ];

        let schema = BundleSchema::initial(&ops);
        assert!(!schema.is_computed(&id_physical), "AttachBlock column is not computed");
        assert!(schema.is_computed(&id_computed), "AddColumn column is computed");
        assert!(!schema.is_computed(&id_unknown), "Unknown column is not computed");
    }

    // --- BundleSchema::blocks_for_column tests ---

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

        let schema = BundleSchema::initial(&ops);
        let blocks = schema.blocks_for_column(&id_a);
        assert_eq!(blocks.len(), 2, "col_a appears in both blocks");

        let blocks_b = schema.blocks_for_column(&id_b);
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

        let schema = BundleSchema::initial(&ops);
        let blocks = schema.blocks_for_column(&id_computed);
        assert_eq!(blocks.len(), 2, "Computed column spans ALL blocks");
    }

    // --- physical_column_name tests ---

    // --- generate_internal_name / parse_internal_name tests ---

    #[test]
    fn test_internal_name_roundtrip() {
        let id = ColumnId::generate();
        let name = generate_internal_name(&id);
        assert!(name.starts_with("col_"));
        let parsed = parse_internal_name(&name).expect("should parse back");
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_parse_internal_name_invalid() {
        assert_eq!(parse_internal_name("not_a_col_id"), None);
        assert_eq!(parse_internal_name("col_"), None);
        assert_eq!(parse_internal_name("col_zzzz"), None);
        assert_eq!(parse_internal_name(""), None);
    }

    // --- BundleSchema::translate_sql tests ---

    #[test]
    fn test_translate_sql_basic() {
        let id_salary = ColumnId::generate();
        let id_name = ColumnId::generate();
        let mut schema = BundleSchema::new();
        schema.insert(id_salary, "salary".to_string());
        schema.insert(id_name, "name".to_string());

        let sql = "SELECT salary, name FROM bundle WHERE salary > 100";
        let result = schema.translate_sql(sql);
        assert!(result.contains(&generate_internal_name(&id_salary)));
        assert!(result.contains(&generate_internal_name(&id_name)));
        assert!(!result.contains("salary"));
        assert!(!result.contains(" name"));
    }

    #[test]
    fn test_translate_sql_avoids_partial_match() {
        let id_id = ColumnId::generate();
        let mut schema = BundleSchema::new();
        schema.insert(id_id, "id".to_string());

        let sql = "SELECT identity, id FROM bundle";
        let result = schema.translate_sql(sql);
        // "identity" should NOT be touched, only "id" should be replaced
        assert!(result.contains("identity"));
        assert!(result.contains(&generate_internal_name(&id_id)));
    }

    #[test]
    fn test_translate_sql_quoted_identifiers() {
        let id_col = ColumnId::generate();
        let mut schema = BundleSchema::new();
        schema.insert(id_col, "my col".to_string());

        let sql = r#"SELECT "my col" FROM bundle"#;
        let result = schema.translate_sql(sql);
        let expected_quoted = format!("\"{}\"", generate_internal_name(&id_col));
        assert!(result.contains(&expected_quoted));
    }
}
