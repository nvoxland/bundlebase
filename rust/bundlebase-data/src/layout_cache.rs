//! LRU cache for loaded PageMaps.
//!
//! Prevents unbounded memory growth when accessing many files by caching
//! loaded layout data with automatic LRU eviction.

use crate::page_map::PageMap;
use lazy_static::lazy_static;
use lru::LruCache;
use opentelemetry::metrics::Counter;
use opentelemetry::KeyValue;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;
use url::Url;

lazy_static! {
    static ref LAYOUT_CACHE_OPS: Counter<u64> = opentelemetry::global::meter("bundlebase")
        .u64_counter("bundlebase.cache.operations")
        .with_description("Cache hits and misses")
        .with_unit("operations")
        .build();
}

/// Global LRU cache for loaded page-group layouts.
///
/// Stores loaded layout data by file URL, with automatic eviction
/// of least-recently-used entries when the cache reaches capacity.
///
/// # Default Capacity
/// - 10,000 files (configurable via environment variable BUNDLEBASE_LAYOUT_CACHE_SIZE)
pub struct LayoutCache {
    cache: Mutex<LruCache<Url, Arc<PageMap>>>,
    evictions: std::sync::atomic::AtomicUsize,
    /// Inserts that replaced an existing entry (same URL, new PageMap).
    /// Distinct from eviction — a replace means the caller didn't check the
    /// cache before loading, so the load was wasted work. Repeated replaces
    /// on the same key are the dominant "cache is busy" signal that
    /// eviction alone misses.
    replaces: std::sync::atomic::AtomicUsize,
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
            evictions: std::sync::atomic::AtomicUsize::new(0),
            replaces: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn get(&self, url: &Url) -> Option<Arc<PageMap>> {
        let result = self.cache.lock().get(url).cloned();
        LAYOUT_CACHE_OPS.add(1, &[
            KeyValue::new("cache_name", "layout_cache"),
            KeyValue::new("result", if result.is_some() { "hit" } else { "miss" }),
        ]);
        result
    }

    pub fn insert(&self, url: Url, layout: Arc<PageMap>) {
        let mut cache = self.cache.lock();
        let already_present = cache.contains(&url);
        let evicting = cache.len() == cache.cap().get() && !already_present;
        if evicting {
            LAYOUT_CACHE_OPS.add(1, &[
                KeyValue::new("cache_name", "layout_cache"),
                KeyValue::new("result", "eviction"),
            ]);
            let total_evictions = self.evictions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if total_evictions == 1 || total_evictions.is_power_of_two() {
                log::warn!(
                    "Layout cache thrashing: {} entries at capacity {}, {} total evictions so far. \
                     Consider increasing BUNDLEBASE_LAYOUT_CACHE_SIZE (current default 10,000).",
                    cache.len(),
                    cache.cap().get(),
                    total_evictions
                );
            }
        } else if already_present {
            // Duplicate insert: caller loaded a layout that was already
            // cached. Count it separately so we can see this specific kind
            // of wasted work — it means a `get()` check was skipped, not
            // that the cache is undersized.
            LAYOUT_CACHE_OPS.add(1, &[
                KeyValue::new("cache_name", "layout_cache"),
                KeyValue::new("result", "replace"),
            ]);
            let total_replaces = self
                .replaces
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            if total_replaces == 1 || total_replaces.is_power_of_two() {
                log::debug!(
                    "Layout cache: {} duplicate inserts so far (caller didn't check before loading)",
                    total_replaces
                );
            }
        }
        cache.put(url, layout);
    }

    /// Returns the total number of evictions since startup.
    pub fn evictions(&self) -> usize {
        self.evictions.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Returns the total number of duplicate inserts since startup.
    pub fn replaces(&self) -> usize {
        self.replaces.load(std::sync::atomic::Ordering::Relaxed)
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
            evictions: self.evictions.load(std::sync::atomic::Ordering::Relaxed),
            replaces: self.replaces.load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub size: usize,
    pub capacity: usize,
    pub evictions: usize,
    pub replaces: usize,
}

impl std::fmt::Display for CacheStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Layout cache: {}/{} entries ({:.1}% full), {} evictions, {} replaces",
            self.size,
            self.capacity,
            (self.size as f64 / self.capacity as f64) * 100.0,
            self.evictions,
            self.replaces,
        )
    }
}

lazy_static! {
    pub static ref GLOBAL_LAYOUT_CACHE: LayoutCache = {
        let capacity = std::env::var("BUNDLEBASE_LAYOUT_CACHE_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(10_000);

        log::debug!("Initializing global layout cache with capacity: {}", capacity);
        LayoutCache::new(capacity)
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_map::PageGroup;

    fn test_layout(total_rows: u64) -> Arc<PageMap> {
        Arc::new(PageMap {
            total_rows,
            file_size: 50000,
            pages: vec![
                PageGroup { physical_start: 0, row_begin: 0 },
                PageGroup { physical_start: 25000, row_begin: total_rows as u32 / 2 },
            ],
            column_stats: vec![],
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
