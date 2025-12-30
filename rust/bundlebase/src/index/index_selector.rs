use crate::data::VersionedBlockId;
use crate::index::IndexDefinition;
use crate::Bundle;
use parking_lot::RwLock;
use std::sync::Arc;

#[cfg(test)]
use crate::index::IndexedBlocks;

/// Determines whether an index should be used for a query
pub struct IndexSelector;

impl IndexSelector {
    /// Select an appropriate index for the given column and block
    ///
    /// Returns Some(IndexDefinition) if:
    /// - An index exists for the column
    /// - The index covers the specified block and version
    ///
    /// Returns None if:
    /// - No index exists for the column
    /// - The index doesn't cover the block
    /// - The block version doesn't match the indexed version (stale index)
    ///
    /// # Arguments
    /// * `column` - The column name to check for an index
    /// * `block` - The VersionedBlockId (block ID + version) to check coverage
    /// * `bundle` - The Bundle with index metadata
    ///
    /// # Example
    /// ```ignore
    /// let versioned_block = VersionedBlockId::new(block_id, version);
    /// if let Some(index_def) = IndexSelector::select_index("email", &versioned_block, &bundle) {
    ///     // Use the index for this query
    /// } else {
    ///     // Fall back to full scan
    /// }
    /// ```
    pub fn select_index(
        column: &str,
        block: &VersionedBlockId,
        bundle: &Bundle,
    ) -> Option<Arc<IndexDefinition>> {
        // Lock and search for an index on the specified column
        let indexes = bundle.indexes().read();

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

    /// Check if any index exists for the given column (regardless of block coverage)
    ///
    /// This is useful for understanding whether an index has been defined,
    /// even if it might not cover all blocks yet.
    #[allow(dead_code)]
    pub fn has_index_for_column(column: &str, bundle: &Bundle) -> bool {
        let indexes = bundle.indexes().read();
        indexes.iter().any(|idx| idx.column() == column)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::ObjectId;

    #[tokio::test]
    async fn test_select_index_found() {
        // Create a test bundle with an index
        let bundle = Bundle::empty().await.expect("Failed to create bundle");

        // Create an IndexDefinition for "email" column
        let index_id = ObjectId::from(1);
        let index_def = Arc::new(IndexDefinition::new(&index_id, &"email".to_string()));

        // Create IndexedBlocks covering block 42 at version "v1"
        let block_id = ObjectId::from(42);
        let version = "v1".to_string();
        let versioned_block = VersionedBlockId::new(block_id.clone(), version.clone());

        let indexed_blocks = Arc::new(IndexedBlocks::from_tuples(
            vec![(block_id.clone(), version.clone())],
            "idx_1.idx".to_string(),
        ));

        index_def.add_indexed_blocks(indexed_blocks);

        // Add index to bundle
        bundle.indexes().write().push(index_def.clone());

        // Test: select_index should find the index
        let result = IndexSelector::select_index("email", &versioned_block, &bundle);
        assert!(result.is_some());

        let found_index = result.unwrap();
        assert_eq!(found_index.column(), "email");
    }

    #[tokio::test]
    async fn test_select_index_wrong_column() {
        // Create a test bundle with an index on "email"
        let bundle = Bundle::empty().await.expect("Failed to create bundle");

        let index_id = ObjectId::from(1);
        let index_def = Arc::new(IndexDefinition::new(&index_id, &"email".to_string()));

        let block_id = ObjectId::from(42);
        let version = "v1".to_string();
        let indexed_blocks = Arc::new(IndexedBlocks::from_tuples(
            vec![(block_id.clone(), version.clone())],
            "idx_1.idx".to_string(),
        ));

        index_def.add_indexed_blocks(indexed_blocks);
        bundle.indexes().write().push(index_def);

        // Test: searching for "name" should return None
        let versioned_block = VersionedBlockId::new(block_id, version);
        let result = IndexSelector::select_index("name", &versioned_block, &bundle);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_select_index_wrong_version() {
        // Create a test bundle with an index at version "v1"
        let bundle = Bundle::empty().await.expect("Failed to create bundle");

        let index_id = ObjectId::from(1);
        let index_def = Arc::new(IndexDefinition::new(&index_id, &"email".to_string()));

        let block_id = ObjectId::from(42);
        let indexed_blocks = Arc::new(IndexedBlocks::from_tuples(
            vec![(block_id.clone(), "v1".to_string())],
            "idx_1.idx".to_string(),
        ));

        index_def.add_indexed_blocks(indexed_blocks);
        bundle.indexes().write().push(index_def);

        // Test: searching with version "v2" should return None (stale index)
        let versioned_block = VersionedBlockId::new(block_id, "v2".to_string());
        let result = IndexSelector::select_index("email", &versioned_block, &bundle);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_has_index_for_column() {
        let bundle = Bundle::empty().await.expect("Failed to create bundle");

        // Initially no indexes
        assert!(!IndexSelector::has_index_for_column("email", &bundle));

        // Add an index
        let index_id = ObjectId::from(1);
        let index_def = Arc::new(IndexDefinition::new(&index_id, &"email".to_string()));
        bundle.indexes().write().push(index_def);

        // Now should find it
        assert!(IndexSelector::has_index_for_column("email", &bundle));
        assert!(!IndexSelector::has_index_for_column("name", &bundle));
    }
}
