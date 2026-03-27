//! LRU cache for loaded PhysicalRowGroupLayouts.
//!
//! Prevents unbounded memory growth when accessing many files by caching
//! loaded layout data with automatic LRU eviction.

use crate::physical_row_group_layout::PhysicalRowGroupLayout;
use lazy_static::lazy_static;
use lru::LruCache;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;
use url::Url;

/// Global LRU cache for loaded page-group layouts.
///
/// Stores loaded layout data by file URL, with automatic eviction
/// of least-recently-used entries when the cache reaches capacity.
///
/// # Default Capacity
/// - 100 files (configurable via environment variable BUNDLEBASE_LAYOUT_CACHE_SIZE)
pub struct LayoutCache {
    cache: Mutex<LruCache<Url, Arc<PhysicalRowGroupLayout>>>,
}

impl LayoutCache {
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

    pub fn get(&self, url: &Url) -> Option<Arc<PhysicalRowGroupLayout>> {
        self.cache.lock().get(url).cloned()
    }

    pub fn insert(&self, url: Url, layout: Arc<PhysicalRowGroupLayout>) {
        let mut cache = self.cache.lock();
        if cache.len() == cache.cap().get() && !cache.contains(&url) {
            log::debug!(
                "Layout cache full ({} entries), evicting LRU entry",
                cache.len()
            );
        }
        cache.put(url, layout);
    }

    pub fn len(&self) -> usize {
        self.cache.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.lock().is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.cache.lock().cap().get()
    }

    pub fn clear(&self) {
        self.cache.lock().clear();
    }

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
            "Layout cache: {}/{} entries ({:.1}% full)",
            self.size,
            self.capacity,
            (self.size as f64 / self.capacity as f64) * 100.0
        )
    }
}

lazy_static! {
    pub static ref GLOBAL_LAYOUT_CACHE: LayoutCache = {
        let capacity = std::env::var("BUNDLEBASE_LAYOUT_CACHE_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(100);

        log::debug!("Initializing global layout cache with capacity: {}", capacity);
        LayoutCache::new(capacity)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physical_row_group_layout::PageGroup;

    fn test_layout(total_rows: u64) -> Arc<PhysicalRowGroupLayout> {
        Arc::new(PhysicalRowGroupLayout {
            total_rows,
            file_size: 50000,
            pages: vec![
                PageGroup { physical_start: 0, row_begin: 0 },
                PageGroup { physical_start: 25000, row_begin: total_rows as u32 / 2 },
            ],
        })
    }

    #[test]
    fn test_cache_basic_operations() {
        let cache = LayoutCache::new(3);
        let url1 = Url::parse("file:///test1.csv").unwrap();
        let url2 = Url::parse("file:///test2.csv").unwrap();

        cache.insert(url1.clone(), test_layout(100));
        let retrieved = cache.get(&url1).unwrap();
        assert_eq!(retrieved.total_rows, 100);
        assert_eq!(cache.len(), 1);

        assert!(cache.get(&url2).is_none());
    }

    #[test]
    fn test_cache_lru_eviction() {
        let cache = LayoutCache::new(2);

        let url1 = Url::parse("file:///test1.csv").unwrap();
        let url2 = Url::parse("file:///test2.csv").unwrap();
        let url3 = Url::parse("file:///test3.csv").unwrap();

        cache.insert(url1.clone(), test_layout(100));
        cache.insert(url2.clone(), test_layout(200));
        assert_eq!(cache.len(), 2);

        assert!(cache.get(&url1).is_some());

        cache.insert(url3.clone(), test_layout(300));
        assert_eq!(cache.len(), 2);

        assert!(cache.get(&url1).is_some());
        assert!(cache.get(&url3).is_some());
        assert!(cache.get(&url2).is_none());
    }

    #[test]
    fn test_cache_capacity() {
        let cache = LayoutCache::new(5);
        assert_eq!(cache.capacity(), 5);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_clear() {
        let cache = LayoutCache::new(3);
        let url1 = Url::parse("file:///test1.csv").unwrap();

        cache.insert(url1, test_layout(100));
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_stats() {
        let cache = LayoutCache::new(10);
        let stats = cache.stats();
        assert_eq!(stats.size, 0);
        assert_eq!(stats.capacity, 10);

        let url = Url::parse("file:///test.csv").unwrap();
        cache.insert(url, test_layout(100));

        let stats = cache.stats();
        assert_eq!(stats.size, 1);
        assert_eq!(stats.capacity, 10);
    }
}
