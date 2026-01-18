use crate::data::RowId;
use crate::metrics;
use lazy_static::lazy_static;
use lru::LruCache;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;
use url::Url;

/// Global LRU cache for loaded RowId indexes to prevent unbounded memory growth.
///
/// This cache stores loaded Vec<RowId> by file URL, with automatic eviction
/// of least-recently-used entries when the cache reaches capacity.
///
/// # Default Capacity
/// - 100 files (configurable via environment variable BUNDLEBASE_ROWID_CACHE_SIZE)
/// - Approximately 80KB per 10,000 rows (8 bytes per RowId)
/// - Max ~80MB for 100 files with 100K rows each
pub struct RowIdCache {
    cache: Mutex<LruCache<Url, Arc<Vec<RowId>>>>,
}

impl RowIdCache {
    /// Creates a new RowIdCache with the specified capacity
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

    /// Gets a cached RowId vector if it exists, promoting it to most-recently-used
    pub fn get(&self, url: &Url) -> Option<Arc<Vec<RowId>>> {
        let result = self.cache.lock().get(url).cloned();

        // Record cache hit/miss metrics
        metrics::record_cache_operation("rowid", result.is_some());

        result
    }

    /// Inserts a RowId vector into the cache, evicting LRU entry if at capacity
    pub fn insert(&self, url: Url, row_ids: Arc<Vec<RowId>>) {
        let mut cache = self.cache.lock();

        // Check if we're evicting an entry
        if cache.len() == cache.cap().get() && !cache.contains(&url) {
            log::debug!(
                "RowId cache full ({} entries), evicting LRU entry",
                cache.len()
            );
        }

        cache.put(url, row_ids);
    }

    /// Returns the current number of cached entries
    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    /// Returns true if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.lock().is_empty()
    }

    /// Returns the maximum capacity of the cache
    pub fn capacity(&self) -> usize {
        self.cache.lock().cap().get()
    }

    /// Clears all entries from the cache
    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    /// Returns cache hit/miss statistics (for monitoring)
    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.lock();
        CacheStats {
            size: cache.len(),
            capacity: cache.cap().get(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub size: usize,
    pub capacity: usize,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RowId cache: {}/{} entries ({:.1}% full)",
            self.size,
            self.capacity,
            (self.size as f64 / self.capacity as f64) * 100.0
        )
    }
}

lazy_static! {
    /// Global singleton RowId cache instance
    ///
    /// Capacity can be configured via BUNDLEBASE_ROWID_CACHE_SIZE environment variable.
    /// Defaults to 100 if not set or invalid.
    pub static ref GLOBAL_ROWID_CACHE: RowIdCache = {
        let capacity = std::env::var("BUNDLEBASE_ROWID_CACHE_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(100);

        log::debug!("Initializing global RowId cache with capacity: {}", capacity);
        RowIdCache::new(capacity)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::ObjectId;

    #[test]
    fn test_cache_basic_operations() {
        let cache = RowIdCache::new(3);
        let url1 = Url::parse("file:///test1.csv").unwrap();
        let url2 = Url::parse("file:///test2.csv").unwrap();

        let row_ids1 = Arc::new(vec![
            RowId::new(&ObjectId::from(1), 0, 100),
            RowId::new(&ObjectId::from(1), 100, 200),
        ]);

        // Insert and retrieve
        cache.insert(url1.clone(), row_ids1.clone());
        let retrieved = cache.get(&url1).unwrap();
        assert_eq!(retrieved.len(), 2);
        assert_eq!(cache.len(), 1);

        // Non-existent key
        assert!(cache.get(&url2).is_none());
    }

    #[test]
    fn test_cache_lru_eviction() {
        let cache = RowIdCache::new(2);
        let block_id = ObjectId::from(1);

        let url1 = Url::parse("file:///test1.csv").unwrap();
        let url2 = Url::parse("file:///test2.csv").unwrap();
        let url3 = Url::parse("file:///test3.csv").unwrap();

        let row_ids1 = Arc::new(vec![RowId::new(&block_id, 0, 10)]);
        let row_ids2 = Arc::new(vec![RowId::new(&block_id, 10, 20)]);
        let row_ids3 = Arc::new(vec![RowId::new(&block_id, 20, 30)]);

        // Fill cache to capacity
        cache.insert(url1.clone(), row_ids1.clone());
        cache.insert(url2.clone(), row_ids2.clone());
        assert_eq!(cache.len(), 2);

        // Access url1 to make it recently used
        assert!(cache.get(&url1).is_some());

        // Insert url3, should evict url2 (LRU)
        cache.insert(url3.clone(), row_ids3.clone());
        assert_eq!(cache.len(), 2);

        // url1 and url3 should be present
        assert!(cache.get(&url1).is_some());
        assert!(cache.get(&url3).is_some());

        // url2 should be evicted
        assert!(cache.get(&url2).is_none());
    }

    #[test]
    fn test_cache_capacity() {
        let cache = RowIdCache::new(5);
        assert_eq!(cache.capacity(), 5);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_clear() {
        let cache = RowIdCache::new(3);
        let url1 = Url::parse("file:///test1.csv").unwrap();
        let row_ids = Arc::new(vec![RowId::new(&ObjectId::from(1), 0, 10)]);

        cache.insert(url1.clone(), row_ids);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_stats() {
        let cache = RowIdCache::new(10);
        let stats = cache.stats();

        assert_eq!(stats.size, 0);
        assert_eq!(stats.capacity, 10);

        let url = Url::parse("file:///test.csv").unwrap();
        let row_ids = Arc::new(vec![RowId::new(&ObjectId::from(1), 0, 10)]);
        cache.insert(url, row_ids);

        let stats = cache.stats();
        assert_eq!(stats.size, 1);
        assert_eq!(stats.capacity, 10);
    }

    #[test]
    fn test_stats_display() {
        let cache = RowIdCache::new(10);
        let url = Url::parse("file:///test.csv").unwrap();
        let row_ids = Arc::new(vec![RowId::new(&ObjectId::from(1), 0, 10)]);
        cache.insert(url, row_ids);

        let stats = cache.stats();
        let display = format!("{}", stats);
        assert!(display.contains("1/10"));
        assert!(display.contains("10.0%"));
    }
}
