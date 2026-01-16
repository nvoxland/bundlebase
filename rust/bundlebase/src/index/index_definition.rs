#![allow(dead_code)]

use crate::data::{ObjectId, VersionedBlockId};
use crate::bundle::IndexedBlocks;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug)]
pub struct IndexDefinition {
    id: ObjectId,
    column: String,
    blocks: RwLock<Vec<Arc<IndexedBlocks>>>, //todo: use BlockIdAndVersion
}

impl IndexDefinition {
    pub(crate) fn new(id: &ObjectId, column: &String) -> IndexDefinition {
        Self {
            id: *id,
            column: column.clone(),
            blocks: RwLock::new(Vec::new()),
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

    /// Returns all indexed blocks for this index definition
    pub(crate) fn all_indexed_blocks(&self) -> Vec<Arc<IndexedBlocks>> {
        self.blocks.read().clone()
    }

    /// Prunes stale indexed blocks that don't match current block versions.
    /// This prevents memory leaks from accumulating old index references.
    ///
    /// # Arguments
    /// * `current_versions` - Map of block IDs to their current versions
    ///
    /// # Returns
    /// Number of stale IndexedBlocks removed
    pub(crate) fn prune_stale_blocks(&self, current_versions: &HashMap<ObjectId, String>) -> usize {
        let mut blocks = self.blocks.write();
        let initial_count = blocks.len();

        blocks.retain(|indexed_blocks| {
            // Keep only blocks where ALL versioned blocks match current versions
            indexed_blocks.blocks().iter().all(|vb| {
                current_versions
                    .get(&vb.block)
                    .map(|current_ver| current_ver == &vb.version)
                    .unwrap_or(false) // Remove if block doesn't exist anymore
            })
        });

        let removed_count = initial_count - blocks.len();

        if removed_count > 0 {
            log::debug!(
                "Pruned {} stale IndexedBlocks from index {} (column '{}')",
                removed_count,
                self.id,
                self.column
            );
        }

        removed_count
    }
}
