use crate::data::{ObjectId, VersionedBlockId};
use parking_lot::RwLock;

/// Represents a set of indexed blocks with their versions.
/// Tracks which blocks (and at which versions) are covered by a particular index.
#[derive(Debug)]
pub struct IndexedBlocks {
    /// List of versioned block IDs that are indexed
    blocks: RwLock<Vec<VersionedBlockId>>,
    /// Path to the index file
    path: String,
}

impl IndexedBlocks {
    /// Creates a new IndexedBlocks instance from a list of VersionedBlockId
    pub fn new(blocks: Vec<VersionedBlockId>, path: String) -> Self {
        Self {
            blocks: RwLock::new(blocks),
            path,
        }
    }

    /// Checks if this index contains the specified block at the specified version
    pub fn contains(&self, block_id: &ObjectId, version: &str) -> bool {
        self.blocks
            .read()
            .iter()
            .any(|vb| &vb.block == block_id && vb.version == version)
    }

    /// Returns the path to the index file
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns a clone of the versioned blocks list
    pub(crate) fn blocks(&self) -> Vec<VersionedBlockId> {
        self.blocks.read().clone()
    }
}
