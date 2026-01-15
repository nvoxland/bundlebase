use crate::data::VersionedBlockId;
use crate::index::IndexDefinition;
use parking_lot::RwLock;
use std::sync::Arc;

/// Determines whether an index should be used for a query
pub struct IndexSelector;

impl IndexSelector {
    /// Select an appropriate index for the given column and block using indexes reference
    ///
    /// This is a convenience method that accepts the indexes RwLock directly,
    /// useful when you have the indexes reference but not the full bundle.
    ///
    /// # Arguments
    /// * `column` - The column name to check for an index
    /// * `block` - The VersionedBlockId (block ID + version) to check coverage
    /// * `indexes` - Reference to the indexes RwLock
    pub fn select_index_from_ref(
        column: &str,
        block: &VersionedBlockId,
        indexes: &Arc<RwLock<Vec<Arc<IndexDefinition>>>>,
    ) -> Option<Arc<IndexDefinition>> {
        let indexes = indexes.read();

        for index_def in indexes.iter() {
            // Check if this index is for the requested column
            if index_def.column() != column {
                continue;
            }

            // Check if this index covers the specified block at the correct version
            if index_def.indexed_blocks(block).is_some() {
                // Found a matching index that covers this block and version
                return Some(index_def.clone());
            }
        }

        // No suitable index found
        None
    }

    // Future enhancement: Add selectivity estimation
    // pub fn estimate_selectivity(
    //     index_def: &IndexDefinition,
    //     predicate: &IndexPredicate,
    // ) -> f64 {
    //     // Return estimated fraction of rows that will match (0.0 to 1.0)
    //     // Could use index cardinality, predicate type, etc.
    //     // Skip index if selectivity > 0.2 (20% of rows)
    // }
}
