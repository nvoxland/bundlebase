//! Filter-pruning helpers for page-level and block-level statistics.
//!
//! Given a filter predicate and a column's pre-computed min/max, determine
//! whether a page (or block) can be skipped entirely.

use bundlebase_index::{IndexedValue, IndexPredicate};
use crate::physical_row_group_layout::StatValue;

/// Returns true if the given exact value is provably outside [page_min, page_max],
/// meaning the page can be skipped.
pub fn prune_exact(
    val: &IndexedValue,
    page_min: Option<&StatValue>,
    page_max: Option<&StatValue>,
) -> bool {
    let (min, max) = match (page_min, page_max) {
        (Some(mn), Some(mx)) => (mn, mx),
        _ => return false, // No stats — can't prune
    };
    let below_min = matches!(min.compare_to_indexed(val), Some(std::cmp::Ordering::Greater));
    let above_max = matches!(max.compare_to_indexed(val), Some(std::cmp::Ordering::Less));
    below_min || above_max
}

/// Returns true if the filter range [filter_min, filter_max] has no overlap with
/// [page_min, page_max], meaning the page can be skipped.
pub fn prune_range(
    filter_min: &IndexedValue,
    filter_max: &IndexedValue,
    page_min: Option<&StatValue>,
    page_max: Option<&StatValue>,
) -> bool {
    let (pmin, pmax) = match (page_min, page_max) {
        (Some(mn), Some(mx)) => (mn, mx),
        _ => return false,
    };
    // No overlap: filter max is below page min, or filter min is above page max
    let filter_above_page = matches!(pmin.compare_to_indexed(filter_max), Some(std::cmp::Ordering::Greater));
    let filter_below_page = matches!(pmax.compare_to_indexed(filter_min), Some(std::cmp::Ordering::Less));
    filter_above_page || filter_below_page
}

/// Extract the upper bound value from a predicate (for monotonic early-stop on increasing columns).
pub fn extract_upper_bound(predicate: &IndexPredicate) -> Option<IndexedValue> {
    match predicate {
        IndexPredicate::Exact(v) => Some(v.clone()),
        IndexPredicate::Range { max, .. } => Some(max.clone()),
        IndexPredicate::In(_) => None,
        IndexPredicate::Prefix(p) => prefix_upper_bound(p).map(IndexedValue::Utf8),
        IndexPredicate::IsNull | IndexPredicate::IsNotNull => None,
    }
}

/// Extract the lower bound value from a predicate (for monotonic early-stop on decreasing columns).
pub fn extract_lower_bound(predicate: &IndexPredicate) -> Option<IndexedValue> {
    match predicate {
        IndexPredicate::Exact(v) => Some(v.clone()),
        IndexPredicate::Range { min, .. } => Some(min.clone()),
        IndexPredicate::In(_) => None,
        IndexPredicate::Prefix(p) => Some(IndexedValue::Utf8(p.clone())),
        IndexPredicate::IsNull | IndexPredicate::IsNotNull => None,
    }
}

/// Compute the exclusive upper bound string for a prefix (e.g., "abc" → "abd").
///
/// Increments the last byte, carrying over 0xFF bytes. Returns `None` if every
/// byte in the prefix is 0xFF (the prefix covers the entire remaining keyspace).
pub fn prefix_upper_bound(prefix: &str) -> Option<String> {
    let mut bytes = prefix.as_bytes().to_vec();
    loop {
        match bytes.last_mut() {
            None => return None,
            Some(b) if *b == 0xFF => { bytes.pop(); }
            Some(b) => { *b += 1; break; }
        }
    }
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Returns true if a page can be pruned for a `LIKE 'prefix%'` predicate.
///
/// Prunes when the page's entire range lies below the prefix or at/above the
/// exclusive upper bound of the prefix.
pub fn prune_prefix(
    prefix: &str,
    page_min: Option<&StatValue>,
    page_max: Option<&StatValue>,
) -> bool {
    let (pmin, pmax) = match (page_min, page_max) {
        (Some(mn), Some(mx)) => (mn, mx),
        _ => return false,
    };
    // Page is entirely below the prefix
    let lower = IndexedValue::Utf8(prefix.to_string());
    if matches!(pmax.compare_to_indexed(&lower), Some(std::cmp::Ordering::Less)) {
        return true;
    }
    // Page is entirely at or above the exclusive upper bound
    if let Some(upper_s) = prefix_upper_bound(prefix) {
        let upper = IndexedValue::Utf8(upper_s);
        if !matches!(pmin.compare_to_indexed(&upper), Some(std::cmp::Ordering::Less)) {
            return true;
        }
    }
    false
}

/// Returns true if `page_min` is strictly above `upper` — meaning pages from here on
/// cannot match an upper-bound filter on a strictly increasing column.
pub fn is_value_above_bound(upper: &IndexedValue, page_min: &StatValue) -> bool {
    matches!(page_min.compare_to_indexed(upper), Some(std::cmp::Ordering::Greater))
}

/// Returns true if `page_max` is strictly below `lower` — meaning pages from here on
/// cannot match a lower-bound filter on a strictly decreasing column.
pub fn is_value_below_bound(lower: &IndexedValue, page_max: &StatValue) -> bool {
    matches!(page_max.compare_to_indexed(lower), Some(std::cmp::Ordering::Less))
}

// FNV-1a constants (must match column_stats_builder)
const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
const FNV_PRIME: u64 = 1_099_511_628_211;
const BLOOM_SIZE_BITS: u64 = 65_536;
const BLOOM_K: usize = 3;

fn fnv1a_hash(data: &[u8], offset: u64) -> u64 {
    let mut hash = offset;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn bloom_positions(value: &str) -> [usize; BLOOM_K] {
    let bytes = value.as_bytes();
    let h1 = fnv1a_hash(bytes, FNV_OFFSET);
    let h2 = fnv1a_hash(bytes, FNV_OFFSET ^ 0x517c_c1b7_2722_0a95);
    [
        (h1 % BLOOM_SIZE_BITS) as usize,
        (h2 % BLOOM_SIZE_BITS) as usize,
        (h1.wrapping_add(h2) % BLOOM_SIZE_BITS) as usize,
    ]
}

/// Returns true if the value MAY be in the bloom filter (probabilistic).
/// Returns false if the value is DEFINITELY NOT in the filter.
///
/// The bloom filter is a 65536-bit array serialized as little-endian u64 words.
/// Uses 3 FNV-1a hash functions (same as `column_stats_builder`).
pub fn bloom_may_contain(bloom_bytes: &[u8], value: &str) -> bool {
    if bloom_bytes.len() < BLOOM_SIZE_BITS as usize / 8 {
        // Corrupted or wrong-size filter — don't prune (be conservative)
        return true;
    }
    for &pos in &bloom_positions(value) {
        let byte_idx = pos / 8;
        let bit_idx = pos % 8;
        if bloom_bytes[byte_idx] & (1 << bit_idx) == 0 {
            return false; // Bit not set → definitely absent
        }
    }
    true // All bits set → may be present
}

/// Returns true if `val` is provably absent from the range or bloom filter — meaning the
/// page/block can be skipped. Checks min/max first; if that passes, checks the bloom filter.
///
/// The bloom filter uses the value's display string (as stored at build time).
pub fn prune_exact_with_bloom(
    val: &IndexedValue,
    page_min: Option<&StatValue>,
    page_max: Option<&StatValue>,
    bloom: Option<&[u8]>,
) -> bool {
    if prune_exact(val, page_min, page_max) {
        return true;
    }
    if let Some(bloom_bytes) = bloom {
        let val_s = indexed_value_display(val);
        return !bloom_may_contain(bloom_bytes, &val_s);
    }
    false
}

/// Check if a block (whole column) can be pruned given an exact value predicate.
pub fn prune_block_exact(
    val: &IndexedValue,
    col_min: Option<&StatValue>,
    col_max: Option<&StatValue>,
) -> bool {
    prune_exact(val, col_min, col_max)
}

/// Check if a block (whole column) can be pruned given a range predicate.
pub fn prune_block_range(
    filter_min: &IndexedValue,
    filter_max: &IndexedValue,
    col_min: Option<&StatValue>,
    col_max: Option<&StatValue>,
) -> bool {
    prune_range(filter_min, filter_max, col_min, col_max)
}

/// Display an `IndexedValue` as a string for bloom filter hashing.
/// Must match the string representation used when the bloom filter was built.
fn indexed_value_display(val: &IndexedValue) -> String {
    match val {
        IndexedValue::Int64(n) => n.to_string(),
        IndexedValue::Float64(f) => f.0.to_string(),
        IndexedValue::Utf8(s) => s.clone(),
        IndexedValue::Boolean(b) => b.to_string(),
        IndexedValue::Timestamp(n) => n.to_string(),
        IndexedValue::Null => String::new(),
    }
}
