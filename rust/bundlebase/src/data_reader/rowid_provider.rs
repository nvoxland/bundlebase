use crate::data_reader::{ObjectId, RowId};
use crate::data_storage::ObjectStoreFile;
use crate::index::RowIdIndex;
use crate::BundlebaseError;
use async_trait::async_trait;
use parking_lot::RwLock;
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

/// CSV-specific RowId provider with pre-loaded layout file caching
/// Loads the entire RowId array from layout file on first access, then caches it
pub struct LayoutRowIdProvider {
    layout: ObjectStoreFile,
    rows: RwLock<Option<Vec<RowId>>>,
}

impl LayoutRowIdProvider {
    pub fn new(layout: ObjectStoreFile) -> Self {
        Self {
            layout,
            rows: RwLock::new(None),
        }
    }
}

#[async_trait]
impl RowIdProvider for LayoutRowIdProvider {
    async fn get_row_ids(&self, begin: usize, end: usize) -> Result<Vec<RowId>, BundlebaseError> {
        // Check cache first
        {
            let cache = self.rows.read();
            if let Some(ref cached) = *cache {
                return Ok(cached[begin..end].to_vec());
            }
        }

        // Not cached, load from layout file
        let index = RowIdIndex::new();
        let loaded =index.load_index(&self.layout).await?;

        // Cache it
        {
            let mut cache = self.rows.write();
            *cache = Some(loaded.clone());
        }

        Ok(loaded[begin..end].to_vec())
    }
}