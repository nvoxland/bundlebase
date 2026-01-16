use crate::data::RowId;
use crate::index::{RowIdIndex, GLOBAL_ROWID_CACHE};
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadFile;
use crate::BundlebaseError;
use async_trait::async_trait;
use std::sync::Arc;

/// Trait for providing RowIds within a specific range
///
/// Different implementations can use different strategies:
/// - Pre-loaded from a layout file with caching (CSV)
/// - Computed on-the-fly based on file metadata (Parquet)
/// - Fetched from an external index service
#[async_trait]
pub trait RowIdProvider: Send + Sync {
    /// Generate RowIds for rows in the range [begin, end)
    ///
    /// Implementations should handle caching efficiently to avoid
    /// redundant loading/computation when called multiple times
    async fn get_row_ids(&self, begin: usize, end: usize) -> Result<Vec<RowId>, BundlebaseError>;
}

/// CSV-specific RowId provider with global LRU caching
///
/// Uses a global LRU cache (GLOBAL_ROWID_CACHE) to prevent unbounded memory growth
/// when accessing many files. The cache automatically evicts least-recently-used
/// entries when it reaches capacity.
pub struct LayoutRowIdProvider {
    layout: ObjectStoreFile,
}

impl LayoutRowIdProvider {
    pub fn new(layout: ObjectStoreFile) -> Self {
        Self { layout }
    }
}

#[async_trait]
impl RowIdProvider for LayoutRowIdProvider {
    async fn get_row_ids(&self, begin: usize, end: usize) -> Result<Vec<RowId>, BundlebaseError> {
        let url = self.layout.url();

        // Check global LRU cache first
        if let Some(cached) = GLOBAL_ROWID_CACHE.get(url) {
            log::trace!("RowId cache hit for {}", url);
            return Ok(cached[begin..end].to_vec());
        }

        // Cache miss - load from layout file
        log::debug!("RowId cache miss for {}, loading from disk", url);
        let index = RowIdIndex::new();
        let loaded = index.load_index(&self.layout).await?;
        let loaded_arc = Arc::new(loaded);

        // Insert into global LRU cache (may evict LRU entry if full)
        GLOBAL_ROWID_CACHE.insert(url.clone(), loaded_arc.clone());

        Ok(loaded_arc[begin..end].to_vec())
    }
}
