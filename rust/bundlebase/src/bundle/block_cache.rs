//! LRU cache for decoded RecordBatches from DataBlock scans.
//!
//! Caches the Arrow RecordBatches produced by full-scan reads so that
//! repeated queries on unchanged data skip all parquet I/O and decoding.
//! Uses a byte-based budget with LRU eviction.
//!
//! Index-based partial reads bypass the cache entirely.

use arrow::record_batch::RecordBatch;
use lazy_static::lazy_static;
use lru::LruCache;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;

/// Default cache budget: 500 MB
const DEFAULT_CACHE_BUDGET_BYTES: usize = 500 * 1024 * 1024;

/// Maximum number of blocks to track in the LRU (entry count limit).
const MAX_CACHE_ENTRIES: usize = 1024;

/// Cached entry for a single block's data.
#[derive(Clone)]
pub struct CachedBlock {
    pub batches: Arc<Vec<RecordBatch>>,
    /// Total memory size of all RecordBatches in bytes.
    pub size_bytes: usize,
}

/// Byte-budget LRU cache for decoded block data.
///
/// Evicts least-recently-used blocks when the total memory exceeds
/// the configured budget. Blocks larger than half the budget are not cached.
pub struct BlockCache {
    inner: Mutex<BlockCacheInner>,
}

struct BlockCacheInner {
    cache: LruCache<String, CachedBlock>,
    /// Current total bytes used by all cached entries.
    current_bytes: usize,
    /// Maximum bytes allowed in the cache.
    budget_bytes: usize,
}

impl BlockCache {
    pub fn new(budget_bytes: usize) -> Self {
        let capacity = NonZeroUsize::new(MAX_CACHE_ENTRIES).expect("non-zero");
        Self {
            inner: Mutex::new(BlockCacheInner {
                cache: LruCache::new(capacity),
                current_bytes: 0,
                budget_bytes,
            }),
        }
    }

    /// Get cached batches for a block, promoting it to most-recently-used.
    pub fn get(&self, key: &str) -> Option<CachedBlock> {
        self.inner.lock().cache.get(key).cloned()
    }

    /// Insert a block's batches into the cache.
    ///
    /// Evicts LRU entries as needed to stay within budget.
    /// Blocks larger than half the budget are silently not cached.
    pub fn insert(&self, key: String, batches: Vec<RecordBatch>) {
        let size_bytes: usize = batches.iter().map(|b| b.get_array_memory_size()).sum();

        let mut inner = self.inner.lock();

        // Don't cache blocks that are too large (> half the budget)
        if size_bytes > inner.budget_bytes / 2 {
            log::debug!(
                "Block {} too large to cache ({} bytes > {} budget/2)",
                key,
                size_bytes,
                inner.budget_bytes / 2
            );
            return;
        }

        // Evict LRU entries until we have room
        while inner.current_bytes + size_bytes > inner.budget_bytes {
            if let Some((evicted_key, evicted)) = inner.cache.pop_lru() {
                inner.current_bytes -= evicted.size_bytes;
                log::debug!(
                    "Evicted block {} from cache ({} bytes, {} total now)",
                    evicted_key,
                    evicted.size_bytes,
                    inner.current_bytes
                );
            } else {
                break;
            }
        }

        // If an old entry exists for this key, subtract its size
        if let Some(old) = inner.cache.pop(&key) {
            inner.current_bytes -= old.size_bytes;
        }

        inner.current_bytes += size_bytes;
        inner.cache.put(
            key,
            CachedBlock {
                batches: Arc::new(batches),
                size_bytes,
            },
        );
    }

    /// Remove a specific block from the cache (used for invalidation).
    pub fn remove(&self, key: &str) {
        let mut inner = self.inner.lock();
        if let Some(entry) = inner.cache.pop(key) {
            inner.current_bytes -= entry.size_bytes;
            log::debug!("Invalidated block {} from cache ({} bytes)", key, entry.size_bytes);
        }
    }

    /// Current total bytes used by the cache.
    pub fn current_bytes(&self) -> usize {
        self.inner.lock().current_bytes
    }

    /// Number of cached blocks.
    pub fn len(&self) -> usize {
        self.inner.lock().cache.len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().cache.is_empty()
    }
}

lazy_static! {
    /// Global singleton block cache instance.
    ///
    /// Budget can be configured via `BUNDLEBASE_BLOCK_CACHE_BYTES` environment variable.
    /// Defaults to 500 MB if not set or invalid.
    pub static ref GLOBAL_BLOCK_CACHE: BlockCache = {
        let budget = std::env::var("BUNDLEBASE_BLOCK_CACHE_BYTES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_CACHE_BUDGET_BYTES);

        log::debug!("Initializing global block cache with {} byte budget", budget);
        BlockCache::new(budget)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Int64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use std::sync::Arc as StdArc;

    fn make_batch(rows: usize) -> RecordBatch {
        let schema = StdArc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)]));
        let array = Int64Array::from((0..rows as i64).collect::<Vec<_>>());
        RecordBatch::try_new(schema, vec![StdArc::new(array)]).expect("batch")
    }

    #[test]
    fn test_basic_cache_hit() {
        let cache = BlockCache::new(10 * 1024 * 1024); // 10MB
        let batch = make_batch(100);
        cache.insert("block1".to_string(), vec![batch.clone()]);
        assert!(cache.get("block1").is_some());
        assert!(cache.get("block2").is_none());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_invalidation() {
        let cache = BlockCache::new(10 * 1024 * 1024);
        cache.insert("block1".to_string(), vec![make_batch(100)]);
        assert_eq!(cache.len(), 1);
        cache.remove("block1");
        assert!(cache.get("block1").is_none());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.current_bytes(), 0);
    }

    #[test]
    fn test_lru_eviction() {
        // Tiny budget: each batch is ~800 bytes, budget allows ~2
        let batch = make_batch(100);
        let batch_size: usize = vec![batch.clone()].iter().map(|b| b.get_array_memory_size()).sum();
        let budget = batch_size * 2 + 1; // Room for 2, not 3
        let cache = BlockCache::new(budget);

        cache.insert("b1".to_string(), vec![make_batch(100)]);
        cache.insert("b2".to_string(), vec![make_batch(100)]);
        assert_eq!(cache.len(), 2);

        // Access b1 to make it recently used
        assert!(cache.get("b1").is_some());

        // Insert b3 — should evict b2 (LRU)
        cache.insert("b3".to_string(), vec![make_batch(100)]);
        assert!(cache.get("b1").is_some());
        assert!(cache.get("b3").is_some());
        assert!(cache.get("b2").is_none());
    }

    #[test]
    fn test_oversized_block_not_cached() {
        let cache = BlockCache::new(1024); // 1KB budget
        let big_batch = make_batch(10_000); // Much larger than 512 bytes (budget/2)
        cache.insert("big".to_string(), vec![big_batch]);
        assert!(cache.get("big").is_none());
        assert_eq!(cache.len(), 0);
    }
}
