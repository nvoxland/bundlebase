use crate::btree_index::BTreeIndex;
use lazy_static::lazy_static;
use lru::LruCache;
use opentelemetry::metrics::Counter;
use opentelemetry::KeyValue;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;

lazy_static! {
    static ref INDEX_CACHE_OPS: Counter<u64> = opentelemetry::global::meter("bundlebase")
        .u64_counter("bundlebase.cache.operations")
        .with_description("Cache hits and misses")
        .with_unit("operations")
        .build();
}

/// Global LRU cache for deserialized `BTreeIndex` objects.
///
/// Indexes are immutable once written (tied to block version), so no
/// invalidation logic is needed — the file path uniquely identifies the
/// content. Caching avoids repeated disk reads and deserialization for
/// hot queries that hit the same indexed column.
///
/// # Default Capacity
/// - 100 entries (configurable via `BUNDLEBASE_INDEX_CACHE_SIZE`)
pub struct IndexCache {
    cache: Mutex<LruCache<String, Arc<BTreeIndex>>>,
}

impl IndexCache {
    /// Creates a new IndexCache with the specified capacity.
    pub fn new(capacity: usize) -> Self {
        let capacity = if let Some(nz) = NonZeroUsize::new(capacity) {
            nz
        } else {
            NonZeroUsize::new(100).expect("100 is non-zero")
        };
        Self {
            cache: Mutex::new(LruCache::new(capacity)),
        }
    }

    /// Gets a cached `BTreeIndex` if it exists, promoting it to most-recently-used.
    pub fn get(&self, index_path: &str) -> Option<Arc<BTreeIndex>> {
        let result = self.cache.lock().get(index_path).cloned();
        INDEX_CACHE_OPS.add(
            1,
            &[
                KeyValue::new("cache_name", "index_cache"),
                KeyValue::new("result", if result.is_some() { "hit" } else { "miss" }),
            ],
        );
        result
    }

    /// Inserts a `BTreeIndex` into the cache, evicting LRU entry if at capacity.
    pub fn insert(&self, index_path: String, index: Arc<BTreeIndex>) {
        let mut cache = self.cache.lock();

        if cache.len() == cache.cap().get() && !cache.contains(&index_path) {
            log::debug!(
                "Index cache full ({} entries), evicting LRU entry",
                cache.len()
            );
        }

        cache.put(index_path, index);
    }

    /// Returns the current number of cached entries.
    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.lock().is_empty()
    }

    /// Returns the maximum capacity of the cache.
    pub fn capacity(&self) -> usize {
        self.cache.lock().cap().get()
    }

    /// Clears all entries from the cache.
    pub fn clear(&self) {
        self.cache.lock().clear();
    }
}

lazy_static! {
    /// Global singleton index cache instance.
    ///
    /// Capacity can be configured via `BUNDLEBASE_INDEX_CACHE_SIZE` environment variable.
    /// Defaults to 100 if not set or invalid.
    pub static ref GLOBAL_INDEX_CACHE: IndexCache = {
        let capacity = std::env::var("BUNDLEBASE_INDEX_CACHE_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(100);

        log::debug!("Initializing global index cache with capacity: {}", capacity);
        IndexCache::new(capacity)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::btree_index::IndexedValue;
    use arrow::datatypes::DataType;
    use bundlebase_common::RowId;
    use std::collections::HashMap;

    fn build_test_index(name: &str) -> Arc<BTreeIndex> {
        let mut value_map = HashMap::new();
        value_map.insert(IndexedValue::Int64(1), vec![RowId::from(100u64)]);
        value_map.insert(IndexedValue::Int64(2), vec![RowId::from(200u64)]);
        Arc::new(BTreeIndex::build(name, &DataType::Int64, value_map).expect("build index"))
    }

    #[test]
    fn test_cache_basic_operations() {
        let cache = IndexCache::new(3);
        let path1 = "block1/index_col1.idx".to_string();
        let path2 = "block2/index_col1.idx".to_string();

        let idx1 = build_test_index("col1");

        cache.insert(path1.clone(), idx1.clone());
        let retrieved = cache.get(&path1).expect("should be cached");
        assert_eq!(retrieved.column_name(), "col1");
        assert_eq!(cache.len(), 1);

        assert!(cache.get(&path2).is_none());
    }

    #[test]
    fn test_cache_lru_eviction() {
        let cache = IndexCache::new(2);

        let path1 = "p1".to_string();
        let path2 = "p2".to_string();
        let path3 = "p3".to_string();

        let idx = build_test_index("c");

        cache.insert(path1.clone(), idx.clone());
        cache.insert(path2.clone(), idx.clone());
        assert_eq!(cache.len(), 2);

        // Access path1 to make it recently used
        assert!(cache.get(&path1).is_some());

        // Insert path3, should evict path2 (LRU)
        cache.insert(path3.clone(), idx.clone());
        assert_eq!(cache.len(), 2);

        assert!(cache.get(&path1).is_some());
        assert!(cache.get(&path3).is_some());
        assert!(cache.get(&path2).is_none());
    }

    #[test]
    fn test_cache_clear() {
        let cache = IndexCache::new(5);
        let idx = build_test_index("c");
        cache.insert("p1".to_string(), idx);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert!(cache.is_empty());
    }
}
