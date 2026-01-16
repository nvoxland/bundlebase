use super::data_block::DataBlock;

use crate::io::ObjectId;
use datafusion::common::JoinType;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::bundle::sql::BASE_PACK_NAME;

/// A pack holds blocks of data and optionally describes a join to the base pack.
///
/// - Base pack (ObjectId 0): No join metadata, contains the primary data
/// - Join packs: Have join metadata (name, expression, join_type) describing how to join with base
#[derive(Debug)]
pub struct Pack {
    id: ObjectId,
    blocks: RwLock<Vec<Arc<DataBlock>>>,
    name: String,

    // Join metadata (all None for base pack, all Some for join packs)
    join_type: Option<JoinTypeOption>,
    expression: Option<String>,
}

impl Clone for Pack {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            blocks: RwLock::new(self.blocks.read().clone()),
            name: self.name.clone(),
            join_type: self.join_type,
            expression: self.expression.clone(),
        }
    }
}

impl Pack {
    pub(crate) fn table_name(id: &ObjectId) -> String {
        format!("__pack_{}", id)
    }

    pub(crate) fn parse_id(table_name: &str) -> Option<ObjectId> {
        // Handle both "packs.__pack_xxx" and "__pack_xxx" formats
        let name = table_name.strip_prefix("packs.").unwrap_or(table_name);
        match name.strip_prefix("__pack_") {
            Some(id) => ObjectId::try_from(id).ok(),
            None => None,
        }
    }

    /// Create the base pack (ObjectId 0) with no join metadata.
    pub fn new_base() -> Self {
        Self {
            id: ObjectId::BASE_PACK,
            blocks: RwLock::new(Vec::new()),
            name: BASE_PACK_NAME.to_string(),
            join_type: None,
            expression: None,
        }
    }

    /// Create a new join pack with join metadata.
    pub fn new(id: ObjectId, name: &str, expression: &str, join_type: JoinTypeOption) -> Self {
        Self {
            id,
            blocks: RwLock::new(Vec::new()),
            name: name.to_string(),
            join_type: Some(join_type),
            expression: Some(expression.to_string()),
        }
    }

    /// Get the pack ID.
    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    /// Check if this is the base pack.
    pub fn is_base(&self) -> bool {
        self.id == ObjectId::BASE_PACK
    }

    /// Check if this pack has join metadata (is a join pack).
    pub fn is_join(&self) -> bool {
        self.expression.is_some()
    }

    /// Get the join name (alias used in SQL expressions).
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// Set the pack name (for rename operations).
    pub fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }

    /// Get the join type.
    pub fn join_type(&self) -> Option<&JoinTypeOption> {
        self.join_type.as_ref()
    }

    /// Get the join expression.
    pub fn expression(&self) -> Option<&str> {
        self.expression.as_deref()
    }

    pub fn add_block(&self, block: Arc<DataBlock>) {
        self.blocks.write().push(block);
    }

    pub fn remove_block(&self, block_id: &ObjectId) {
        self.blocks.write().retain(|b| b.id() != block_id);
    }

    pub fn blocks(&self) -> Vec<Arc<DataBlock>> {
        self.blocks.read().clone()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.read().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JoinTypeOption {
    Inner,
    Left,
    Right,
    Full,
}

impl JoinTypeOption {
    pub fn to_datafusion(&self) -> JoinType {
        match self {
            JoinTypeOption::Inner => JoinType::Inner,
            JoinTypeOption::Left => JoinType::Left,
            JoinTypeOption::Right => JoinType::Right,
            JoinTypeOption::Full => JoinType::Full,
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "left" => JoinTypeOption::Left,
            "right" => JoinTypeOption::Right,
            "full" | "outer" => JoinTypeOption::Full,
            _ => JoinTypeOption::Inner,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            JoinTypeOption::Inner => "inner",
            JoinTypeOption::Left => "left",
            JoinTypeOption::Right => "right",
            JoinTypeOption::Full => "full",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_name() {
        let id = ObjectId::from(58);
        let table = Pack::table_name(&id);
        assert_eq!(table, "__pack_3a");
    }

    #[test]
    fn test_new_base() {
        let pack = Pack::new_base();
        assert!(pack.is_base());
        assert!(!pack.is_join());
        assert_eq!(pack.name(), BASE_PACK_NAME);
    }

    #[test]
    fn test_new_join() {
        let id = ObjectId::generate();
        let pack = Pack::new(
            id,
            "customers",
            "base.id = customers.id",
            JoinTypeOption::Left,
        );
        assert!(!pack.is_base());
        assert!(pack.is_join());
        assert_eq!(pack.name(), "customers");
        assert_eq!(pack.expression(), Some("base.id = customers.id"));
        assert_eq!(pack.join_type(), Some(&JoinTypeOption::Left));
    }

    #[test]
    fn test_join_type_from_str() {
        assert_eq!(JoinTypeOption::from_str("inner"), JoinTypeOption::Inner);
        assert_eq!(JoinTypeOption::from_str("left"), JoinTypeOption::Left);
        assert_eq!(JoinTypeOption::from_str("right"), JoinTypeOption::Right);
        assert_eq!(JoinTypeOption::from_str("full"), JoinTypeOption::Full);
        assert_eq!(JoinTypeOption::from_str("outer"), JoinTypeOption::Full);
        assert_eq!(JoinTypeOption::from_str("unknown"), JoinTypeOption::Inner);
    }

    #[test]
    fn test_join_type_as_str() {
        assert_eq!(JoinTypeOption::Inner.as_str(), "inner");
        assert_eq!(JoinTypeOption::Left.as_str(), "left");
        assert_eq!(JoinTypeOption::Right.as_str(), "right");
        assert_eq!(JoinTypeOption::Full.as_str(), "full");
    }

    #[test]
    fn test_join_type_to_datafusion() {
        assert_eq!(JoinTypeOption::Inner.to_datafusion(), JoinType::Inner);
        assert_eq!(JoinTypeOption::Left.to_datafusion(), JoinType::Left);
        assert_eq!(JoinTypeOption::Right.to_datafusion(), JoinType::Right);
        assert_eq!(JoinTypeOption::Full.to_datafusion(), JoinType::Full);
    }

    #[test]
    fn test_join_type_eq() {
        assert_eq!(JoinTypeOption::Inner, JoinTypeOption::Inner);
        assert_ne!(JoinTypeOption::Inner, JoinTypeOption::Left);
        assert_ne!(JoinTypeOption::Full, JoinTypeOption::Right);
    }
}
