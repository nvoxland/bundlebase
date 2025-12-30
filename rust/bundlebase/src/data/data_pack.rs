use crate::data::data_block::DataBlock;

use crate::io::ObjectId;
use parking_lot::RwLock;
use std::sync::Arc;

#[derive(Debug)]
pub struct DataPack {
    id: ObjectId,
    blocks: RwLock<Vec<Arc<DataBlock>>>,
}

impl Clone for DataPack {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            blocks: RwLock::new(self.blocks.read().clone()),
        }
    }
}

impl DataPack {
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

    pub fn new(id: ObjectId) -> Self {
        Self {
            id,
            blocks: RwLock::new(Vec::new()),
        }
    }

    pub fn add_block(&self, block: Arc<DataBlock>) {
        self.blocks.write().push(block);
    }

    pub fn blocks(&self) -> Vec<Arc<DataBlock>> {
        self.blocks.read().clone()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.read().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_name() {
        let id = ObjectId::from(58);
        let table = DataPack::table_name(&id);
        assert_eq!(table, "__pack_3a");
    }
}
