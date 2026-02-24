use crate::bundle::operation::AnyOperation;
use crate::object_id::ColumnId;
use std::collections::HashMap;

/// A computed registry mapping column IDs to current logical names and vice versa.
///
/// Built by iterating over a bundle's operations. Not stored — always recomputed
/// from the operation list. This gives columns stable identity that survives renames.
#[derive(Debug, Clone)]
pub struct ColumnRegistry {
    /// id → current logical name
    columns: HashMap<ColumnId, String>,
    /// current logical name → id
    names: HashMap<String, ColumnId>,
    /// id → original name (at registration time, never changes)
    original_names: HashMap<ColumnId, String>,
}

impl ColumnRegistry {
    pub fn new() -> Self {
        Self {
            columns: HashMap::new(),
            names: HashMap::new(),
            original_names: HashMap::new(),
        }
    }

    /// Build a ColumnRegistry from a list of operations.
    pub fn from_operations(operations: &[AnyOperation]) -> Self {
        let mut registry = Self::new();

        for op in operations {
            match op {
                AnyOperation::AttachBlock(attach) => {
                    if let Some(schema) = &attach.schema {
                        for (field, id) in schema.fields().iter().zip(attach.column_ids.iter()) {
                            registry.register(*id, field.name());
                        }
                    }
                }
                AnyOperation::AddColumn(add) => {
                    registry.register(add.id, &add.name);
                }
                AnyOperation::RenameColumn(rename) => {
                    registry.rename_by_id(&rename.id, &rename.new_name);
                }
                AnyOperation::DropColumn(drop) => {
                    registry.drop_by_id(&drop.id);
                }
                AnyOperation::CastColumn(cast) => {
                    // Cast doesn't change name, but record the ID if not already present
                    if !registry.columns.contains_key(&cast.id) {
                        registry.register(cast.id, &cast.name);
                    }
                }
                _ => {}
            }
        }

        registry
    }

    /// Register a column ID with a name.
    pub fn register(&mut self, id: ColumnId, name: &str) {
        // Don't overwrite an existing registration for this name
        // (first attach wins — subsequent attaches of blocks with the same column
        // name share the ID from the first)
        if self.names.contains_key(name) {
            return;
        }
        self.columns.insert(id, name.to_string());
        self.names.insert(name.to_string(), id);
        self.original_names.entry(id).or_insert_with(|| name.to_string());
    }

    /// Rename a column by its old logical name.
    pub fn rename(&mut self, old_name: &str, new_name: &str) {
        if let Some(id) = self.names.remove(old_name) {
            self.columns.insert(id, new_name.to_string());
            self.names.insert(new_name.to_string(), id);
        }
    }

    /// Rename a column by its ID.
    fn rename_by_id(&mut self, id: &ColumnId, new_name: &str) {
        if let Some(old_name) = self.columns.get(id).cloned() {
            self.names.remove(&old_name);
        }
        self.columns.insert(*id, new_name.to_string());
        self.names.insert(new_name.to_string(), *id);
    }

    /// Drop a column by name.
    pub fn drop_column(&mut self, name: &str) {
        if let Some(id) = self.names.remove(name) {
            self.columns.remove(&id);
        }
    }

    /// Drop a column by ID.
    fn drop_by_id(&mut self, id: &ColumnId) {
        if let Some(name) = self.columns.remove(id) {
            self.names.remove(&name);
        }
    }

    /// Look up a column ID by its current logical name.
    pub fn id_for_name(&self, name: &str) -> Option<ColumnId> {
        self.names.get(name).copied()
    }

    /// Look up the current logical name for a column ID.
    pub fn name_for_id(&self, id: &ColumnId) -> Option<&str> {
        self.columns.get(id).map(|s| s.as_str())
    }

    /// Look up the original name for a column ID (the name it was first registered with).
    pub fn original_name_for_id(&self, id: &ColumnId) -> Option<&str> {
        self.original_names.get(id).map(|s| s.as_str())
    }

    /// Check if the registry is empty (no columns registered).
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_lookup() {
        let mut registry = ColumnRegistry::new();
        let id = ColumnId::generate();
        registry.register(id, "name");

        assert_eq!(registry.id_for_name("name"), Some(id));
        assert_eq!(registry.name_for_id(&id), Some("name"));
    }

    #[test]
    fn test_rename() {
        let mut registry = ColumnRegistry::new();
        let id = ColumnId::generate();
        registry.register(id, "old_name");
        registry.rename("old_name", "new_name");

        assert_eq!(registry.id_for_name("old_name"), None);
        assert_eq!(registry.id_for_name("new_name"), Some(id));
        assert_eq!(registry.name_for_id(&id), Some("new_name"));
    }

    #[test]
    fn test_drop() {
        let mut registry = ColumnRegistry::new();
        let id = ColumnId::generate();
        registry.register(id, "col");
        registry.drop_column("col");

        assert_eq!(registry.id_for_name("col"), None);
        assert_eq!(registry.name_for_id(&id), None);
        assert!(registry.is_empty());
    }

    #[test]
    fn test_rename_nonexistent_is_noop() {
        let mut registry = ColumnRegistry::new();
        registry.rename("nonexistent", "new_name");
        assert!(registry.is_empty());
    }

    #[test]
    fn test_drop_nonexistent_is_noop() {
        let mut registry = ColumnRegistry::new();
        registry.drop_column("nonexistent");
        assert!(registry.is_empty());
    }

    #[test]
    fn test_register_duplicate_name_is_noop() {
        let mut registry = ColumnRegistry::new();
        let id1 = ColumnId::generate();
        let id2 = ColumnId::generate();
        registry.register(id1, "col");
        registry.register(id2, "col");

        // First registration wins
        assert_eq!(registry.id_for_name("col"), Some(id1));
    }
}
