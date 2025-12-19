use std::sync::Arc;
use parking_lot::RwLock;
use crate::data_reader::{ObjectId, VersionedBlockId};
use crate::index::indexed_blocks::IndexedBlocks;

#[derive(Debug)]
pub struct IndexDefinition {
    id: ObjectId,
    column: String,
    blocks: RwLock<Vec<Arc<IndexedBlocks>>>, //todo: use BlockIdAndVersion
}

impl IndexDefinition {
    pub(crate) fn new(id: &ObjectId, column: &String) -> IndexDefinition {
        Self {
            id: id.clone(),
            column: column.clone(),
            blocks: RwLock::new(Vec::new())
        }
    }

    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    pub fn column(&self) -> &String {
        &self.column
    }

    pub fn indexed_blocks(&self, versioned_block: &VersionedBlockId) -> Option<Arc<IndexedBlocks>> {
        for blocks in self.blocks.read().iter() {
            if blocks.contains(&versioned_block.block, &versioned_block.version) {
                return Some(blocks.clone());
            }
        }
        None
    }

    /// Adds a new set of indexed blocks to this index definition
    pub(crate) fn add_indexed_blocks(&self, indexed_blocks: Arc<IndexedBlocks>) {
        self.blocks.write().push(indexed_blocks);
    }

}
