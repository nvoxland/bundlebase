//! Per-column statistics accumulator for CSV/JSONL attach-time profiling.
//!
//! Processes Arrow RecordBatches (with all columns as Utf8) to accumulate:
//! - null count, min, max (string representation)
//! - top 10 most common values (with counts)
//! - HyperLogLog distinct count (b=14, ~0.8% error at any cardinality)
//! - is_all_numeric: every non-null value parses as f64
//! - is_strictly_increasing / is_strictly_decreasing:
//!   numeric comparison when is_all_numeric, lexicographic otherwise
//! - per-page min/max and distinct count
//! - string profile (min/max/avg length, % ASCII) for non-numeric columns
//! - 10-bucket equal-height (quantile) histogram for both numeric and string columns
//! - per-page bloom filter (8KB, 3-hash FNV1a; abandoned when page has ≥500 distinct values)

use crate::physical_row_group_layout::{ColumnStats, HistogramBucket, PageStats, StatValue, StringProfile};
use arrow::array::{Array, StringArray};
use arrow::record_batch::RecordBatch;
use std::collections::{HashMap, HashSet};

/// Cap for the top-candidates HashMap before pruning.
const TOP_CANDIDATES_CAP: usize = 2000;
/// How many candidates to keep after pruning.
const TOP_CANDIDATES_PRUNE_TO: usize = 1000;
/// Final number of top values to keep in ColumnStats.
const TOP_VALUES_COUNT: usize = 10;
/// Max reservoir samples for histogram building.
const HISTOGRAM_SAMPLE_CAP: usize = 1000;
/// Number of histogram buckets.
const HISTOGRAM_BUCKETS: usize = 10;
/// Bloom filter size in bits (8KB).
const BLOOM_SIZE_BITS: u64 = 65_536;
/// Bloom filter backing array size in u64s.
const BLOOM_SIZE_U64S: usize = (BLOOM_SIZE_BITS / 64) as usize; // 1024
/// Number of hash functions for the bloom filter.
const BLOOM_K: usize = 3;
/// Max distinct values tracked per page.
const PAGE_DISTINCT_CAP: usize = 500;
/// HyperLogLog register count log2 (b=14 → 16384 registers, ~0.8% error).
const HLL_B: u32 = 14;
const HLL_M: usize = 1 << HLL_B; // 16384

// FNV-1a constants (64-bit)
const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
const FNV_PRIME: u64 = 1_099_511_628_211;

/// FNV-1a hash with a custom offset (used to generate multiple independent hash functions).
fn fnv1a_hash(data: &[u8], offset: u64) -> u64 {
    let mut hash = offset;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// HyperLogLog sketch for approximate distinct counting.
///
/// Uses b=14 (16384 registers) for ~0.8% relative error. Each register stores
/// the maximum position of the first 1-bit seen in hashes routed to it.
struct HyperLogLog {
    registers: Vec<u8>,
}

impl HyperLogLog {
    fn new() -> Self {
        Self { registers: vec![0u8; HLL_M] }
    }

    fn add(&mut self, value: &str) {
        let h = fnv1a_hash(value.as_bytes(), FNV_OFFSET);
        let idx = (h >> (64 - HLL_B)) as usize;
        let w = h << HLL_B;
        // rho = position of leftmost 1 bit in the remaining bits (1-indexed)
        let rho = if w == 0 { (64 - HLL_B) as u8 + 1 } else { w.leading_zeros() as u8 + 1 };
        if rho > self.registers[idx] {
            self.registers[idx] = rho;
        }
    }

    fn estimate(&self) -> u64 {
        let m = HLL_M as f64;
        // alpha_m constant for b=14
        let alpha = 0.7213 / (1.0 + 1.079 / m);
        let sum: f64 = self.registers.iter().map(|&r| 2.0f64.powi(-(r as i32))).sum();
        let raw = alpha * m * m / sum;

        // Small range: linear counting
        if raw < 2.5 * m {
            let zeros = self.registers.iter().filter(|&&r| r == 0).count() as f64;
            if zeros > 0.0 {
                return (m * (m / zeros).ln()).round() as u64;
            }
        }
        // Large range correction
        let two_32 = (1u64 << 32) as f64;
        if raw > two_32 / 30.0 {
            return ((-two_32) * (1.0 - raw / two_32).ln()).round() as u64;
        }

        raw.round() as u64
    }
}

/// Compute the 3 bit positions for inserting/querying a value in the bloom filter.
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

/// Accumulates statistics for a single column across multiple batches.
struct ColumnAccumulator {
    null_count: u64,
    min: Option<String>,
    max: Option<String>,
    top_candidates: HashMap<String, u64>,
    is_all_numeric: bool,
    // Numeric monotonic tracking (used while is_all_numeric remains true)
    is_numeric_increasing: bool,
    is_numeric_decreasing: bool,
    last_numeric: Option<f64>,
    // String monotonic tracking (always maintained as fallback)
    is_str_increasing: bool,
    is_str_decreasing: bool,
    last_str: Option<String>,
    any_non_null_seen: bool,
    // Per-page min/max (positional: index = page index)
    page_mins: Vec<Option<String>>,
    page_maxs: Vec<Option<String>>,
    // Per-page distinct count (HashSet capped at PAGE_DISTINCT_CAP per page)
    page_distinct: Vec<HashSet<String>>,
    // String profile fields (always tracked; only emitted for non-numeric columns)
    total_len: u64,
    len_count: u64,
    min_len: Option<u32>,
    max_len: Option<u32>,
    non_ascii_count: u64,
    // Histogram reservoir (deterministic reservoir sample, up to HISTOGRAM_SAMPLE_CAP)
    histogram_samples: Vec<String>,
    // Per-page bloom filter: Some = active, None = abandoned (>= PAGE_DISTINCT_CAP distinct values)
    page_bloom_bits: Vec<Option<Vec<u64>>>,
    // HyperLogLog sketch for accurate distinct count at any cardinality
    hll: HyperLogLog,
}

impl ColumnAccumulator {
    fn new(page_count: usize) -> Self {
        let page_count = page_count.max(1);
        Self {
            null_count: 0,
            min: None,
            max: None,
            top_candidates: HashMap::new(),
            is_all_numeric: true,
            is_numeric_increasing: true,
            is_numeric_decreasing: true,
            last_numeric: None,
            is_str_increasing: true,
            is_str_decreasing: true,
            last_str: None,
            any_non_null_seen: false,
            page_mins: vec![None; page_count],
            page_maxs: vec![None; page_count],
            page_distinct: (0..page_count).map(|_| HashSet::new()).collect(),
            total_len: 0,
            len_count: 0,
            min_len: None,
            max_len: None,
            non_ascii_count: 0,
            histogram_samples: Vec::new(),
            page_bloom_bits: (0..page_count).map(|_| Some(vec![0u64; BLOOM_SIZE_U64S])).collect(),
            hll: HyperLogLog::new(),
        }
    }

    fn update(&mut self, value: &str, page_idx: usize) {
        // Global min / max
        match &self.min {
            None => self.min = Some(value.to_string()),
            Some(current) if value < current.as_str() => self.min = Some(value.to_string()),
            _ => {}
        }
        match &self.max {
            None => self.max = Some(value.to_string()),
            Some(current) if value > current.as_str() => self.max = Some(value.to_string()),
            _ => {}
        }

        // Per-page min / max
        if page_idx < self.page_mins.len() {
            match &self.page_mins[page_idx] {
                None => self.page_mins[page_idx] = Some(value.to_string()),
                Some(m) if value < m.as_str() => self.page_mins[page_idx] = Some(value.to_string()),
                _ => {}
            }
            match &self.page_maxs[page_idx] {
                None => self.page_maxs[page_idx] = Some(value.to_string()),
                Some(m) if value > m.as_str() => self.page_maxs[page_idx] = Some(value.to_string()),
                _ => {}
            }
        }

        // Per-page distinct count (track up to PAGE_DISTINCT_CAP unique values)
        if page_idx < self.page_distinct.len()
            && self.page_distinct[page_idx].len() < PAGE_DISTINCT_CAP
        {
            self.page_distinct[page_idx].insert(value.to_string());
        }

        // Per-page bloom filter: insert value if the page hasn't hit the distinct cap yet.
        // Abandon when page_distinct is full (>= PAGE_DISTINCT_CAP distinct values seen).
        if page_idx < self.page_bloom_bits.len() {
            if self.page_distinct[page_idx].len() >= PAGE_DISTINCT_CAP {
                self.page_bloom_bits[page_idx] = None;
            } else if let Some(ref mut bits) = self.page_bloom_bits[page_idx] {
                for pos in bloom_positions(value) {
                    bits[pos / 64] |= 1u64 << (pos % 64);
                }
            }
        }

        // HyperLogLog distinct count
        self.hll.add(value);

        // Top candidates
        *self.top_candidates.entry(value.to_string()).or_insert(0) += 1;
        if self.top_candidates.len() > TOP_CANDIDATES_CAP {
            self.prune_top_candidates();
        }

        // String profile: length and ASCII tracking
        let len = value.len() as u32;
        self.total_len += len as u64;
        self.len_count += 1;
        self.min_len = Some(self.min_len.map_or(len, |m| m.min(len)));
        self.max_len = Some(self.max_len.map_or(len, |m| m.max(len)));
        if !value.is_ascii() {
            self.non_ascii_count += 1;
        }

        // Histogram reservoir sampling (deterministic; uses FNV hash as pseudo-random source)
        if self.histogram_samples.len() < HISTOGRAM_SAMPLE_CAP {
            self.histogram_samples.push(value.to_string());
        } else {
            // Reservoir sample: replace a random slot with probability HISTOGRAM_SAMPLE_CAP / n
            let n = self.len_count as usize;
            let slot = fnv1a_hash(value.as_bytes(), n as u64) as usize % n;
            if slot < HISTOGRAM_SAMPLE_CAP {
                self.histogram_samples[slot] = value.to_string();
            }
        }

        // Numeric check (short-circuit once false)
        let numeric_val = if self.is_all_numeric {
            match value.parse::<f64>() {
                Ok(n) => Some(n),
                Err(_) => {
                    self.is_all_numeric = false;
                    self.is_numeric_increasing = false;
                    self.is_numeric_decreasing = false;
                    None
                }
            }
        } else {
            None
        };

        // Numeric monotonic tracking
        if let Some(n) = numeric_val {
            if let Some(last) = self.last_numeric {
                if n <= last {
                    self.is_numeric_increasing = false;
                }
                if n >= last {
                    self.is_numeric_decreasing = false;
                }
            }
            self.last_numeric = Some(n);
        }

        // String monotonic tracking (always)
        if self.is_str_increasing || self.is_str_decreasing {
            if let Some(ref last) = self.last_str.clone() {
                if value <= last.as_str() {
                    self.is_str_increasing = false;
                }
                if value >= last.as_str() {
                    self.is_str_decreasing = false;
                }
            }
        }
        self.last_str = Some(value.to_string());
        self.any_non_null_seen = true;
    }

    fn prune_top_candidates(&mut self) {
        let mut entries: Vec<(String, u64)> = self.top_candidates.drain().collect();
        entries.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(TOP_CANDIDATES_PRUNE_TO);
        self.top_candidates = entries.into_iter().collect();
    }

    fn finish(mut self) -> ColumnStats {
        // HyperLogLog gives accurate distinct counts at all cardinalities (~0.8% error)
        let distinct_count = self.hll.estimate();

        // Build top_values: take top N by count
        let mut entries: Vec<(String, u64)> = self.top_candidates.drain().collect();
        entries.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(TOP_VALUES_COUNT);

        let (is_strictly_increasing, is_strictly_decreasing) = if self.is_all_numeric {
            (self.is_numeric_increasing, self.is_numeric_decreasing)
        } else {
            (self.is_str_increasing, self.is_str_decreasing)
        };

        // Only claim monotonicity when there are at least 2 non-null values to compare.
        let has_ordering = self.last_str.is_some() && self.any_non_null_seen;
        let (is_strictly_increasing, is_strictly_decreasing) = if has_ordering {
            (is_strictly_increasing, is_strictly_decreasing)
        } else {
            (false, false)
        };

        let is_numeric = self.is_all_numeric;

        // Per-page stats (including per-page bloom filters)
        let page_stats = self.page_mins.into_iter()
            .zip(self.page_maxs.into_iter())
            .zip(self.page_distinct.into_iter())
            .zip(self.page_bloom_bits.into_iter())
            .map(|(((min, max), distinct_set), bloom_bits)| {
                let bloom_filter = bloom_bits.map(|bits| {
                    bits.iter().flat_map(|&word| word.to_le_bytes()).collect()
                });
                PageStats {
                    min: min.map(|s| str_to_stat_value(s, is_numeric)),
                    max: max.map(|s| str_to_stat_value(s, is_numeric)),
                    distinct_count: distinct_set.len() as u64,
                    bloom_filter,
                }
            })
            .collect();

        // String profile (only for non-numeric columns with at least one non-null value)
        let string_profile = if !self.is_all_numeric && self.len_count > 0 {
            Some(StringProfile {
                min_len: self.min_len.unwrap_or(0),
                max_len: self.max_len.unwrap_or(0),
                avg_len: self.total_len as f32 / self.len_count as f32,
                pct_ascii: 1.0 - (self.non_ascii_count as f32 / self.len_count as f32),
            })
        } else {
            None
        };

        // Histogram
        let histogram = build_histogram(&mut self.histogram_samples, self.is_all_numeric);

        ColumnStats {
            null_count: self.null_count,
            min: self.min.map(|s| str_to_stat_value(s, is_numeric)),
            max: self.max.map(|s| str_to_stat_value(s, is_numeric)),
            top_values: entries,
            is_all_numeric: self.is_all_numeric,
            is_strictly_increasing,
            is_strictly_decreasing,
            distinct_count,
            page_stats,
            string_profile,
            histogram,
        }
    }
}

/// Convert a string value to a typed `StatValue`.
///
/// CSV/JSONL data arrives as Utf8; numeric columns are stored as `Float64` so
/// typed comparisons work during pruning. Falls back to `Utf8` if parsing fails.
fn str_to_stat_value(s: String, is_numeric: bool) -> StatValue {
    if is_numeric {
        s.parse::<f64>().map(StatValue::Float64).unwrap_or_else(|_| StatValue::Utf8(s))
    } else {
        StatValue::Utf8(s)
    }
}

/// Build a 10-bucket equal-height (quantile) histogram from a reservoir sample.
///
/// Both numeric and string columns use equal-height buckets: sort the samples and
/// divide into deciles. For numeric columns the bucket boundaries are Float64 values;
/// for string columns they are Utf8 values.
fn build_histogram(samples: &mut Vec<String>, is_all_numeric: bool) -> Vec<HistogramBucket> {
    if samples.is_empty() {
        return vec![];
    }

    if is_all_numeric {
        let mut numeric: Vec<f64> = samples.iter().filter_map(|s| s.parse().ok()).collect();
        if numeric.is_empty() {
            return vec![];
        }
        numeric.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = numeric.len();
        let mut buckets = Vec::with_capacity(HISTOGRAM_BUCKETS);
        let mut prev_idx = 0;
        for i in 1..=HISTOGRAM_BUCKETS {
            let next_idx = (n * i / HISTOGRAM_BUCKETS).min(n);
            if prev_idx >= n {
                break;
            }
            buckets.push(HistogramBucket {
                lower_bound: StatValue::Float64(numeric[prev_idx]),
                count: (next_idx - prev_idx) as u64,
            });
            prev_idx = next_idx;
        }
        buckets
    } else {
        samples.sort_unstable();
        let n = samples.len();
        let mut buckets = Vec::with_capacity(HISTOGRAM_BUCKETS);
        let mut prev_idx = 0;
        for i in 1..=HISTOGRAM_BUCKETS {
            let next_idx = (n * i / HISTOGRAM_BUCKETS).min(n);
            if prev_idx >= n {
                break;
            }
            buckets.push(HistogramBucket {
                lower_bound: StatValue::Utf8(samples[prev_idx].clone()),
                count: (next_idx - prev_idx) as u64,
            });
            prev_idx = next_idx;
        }
        buckets
    }
}

/// Accumulate statistics across multiple RecordBatches.
///
/// All columns must be `Utf8` (string) type — pass the all-Utf8 schema to DataFusion
/// when creating the reader. Returns one `ColumnStats` per column, positional.
///
/// `page_row_starts` should be `layout.pages[i].row_begin` for each page. If empty,
/// the entire file is treated as a single page (index 0).
pub struct ColumnStatsBuilder {
    accumulators: Vec<ColumnAccumulator>,
    /// Sorted list of row numbers where each page begins. Empty = single-page mode.
    page_row_starts: Vec<u32>,
    /// The current logical row number across all processed rows.
    current_row: u64,
}

impl ColumnStatsBuilder {
    pub fn new(column_count: usize, page_row_starts: &[u32]) -> Self {
        let page_count = if page_row_starts.is_empty() { 1 } else { page_row_starts.len() };
        Self {
            accumulators: (0..column_count).map(|_| ColumnAccumulator::new(page_count)).collect(),
            page_row_starts: page_row_starts.to_vec(),
            current_row: 0,
        }
    }

    /// Find which page a given row number belongs to. Returns 0 if no pages defined.
    fn find_page_for_row(&self, row: u64) -> usize {
        if self.page_row_starts.is_empty() {
            return 0;
        }
        match self.page_row_starts.binary_search(&(row as u32)) {
            Ok(idx) => idx,
            Err(idx) => if idx == 0 { 0 } else { idx - 1 },
        }
    }

    /// Process one RecordBatch. All columns must be StringArray.
    pub fn process_batch(&mut self, batch: &RecordBatch) {
        let batch_start_row = self.current_row;
        let num_rows = batch.num_rows();

        // Precompute page index for each row in the batch (avoids repeated binary searches
        // per column; rows are processed in order so we advance a cursor through pages).
        let page_indices: Vec<usize> = {
            let mut indices = Vec::with_capacity(num_rows);
            let mut cur_page = self.find_page_for_row(batch_start_row);
            for i in 0..num_rows {
                let row = batch_start_row + i as u64;
                while cur_page + 1 < self.page_row_starts.len()
                    && row >= self.page_row_starts[cur_page + 1] as u64
                {
                    cur_page += 1;
                }
                indices.push(cur_page);
            }
            indices
        };

        for (col_idx, acc) in self.accumulators.iter_mut().enumerate() {
            let array = batch.column(col_idx);
            let string_array = match array.as_any().downcast_ref::<StringArray>() {
                Some(a) => a,
                None => continue,
            };
            for (row_i, &page_idx) in page_indices.iter().enumerate() {
                if string_array.is_null(row_i) {
                    acc.null_count += 1;
                } else {
                    acc.update(string_array.value(row_i), page_idx);
                }
            }
        }

        self.current_row += num_rows as u64;
    }

    /// Process one JSONL row (parsed JSON object).
    ///
    /// `name_to_idx` maps column name → column index. Columns not present in the
    /// JSON object are counted as null. Values are converted to their string form.
    pub fn process_jsonl_row(
        &mut self,
        obj: &serde_json::Map<String, serde_json::Value>,
        name_to_idx: &std::collections::HashMap<&str, usize>,
        col_count: usize,
    ) {
        let page_idx = self.find_page_for_row(self.current_row);

        let mut seen = vec![false; col_count];

        for (key, val) in obj {
            if let Some(&idx) = name_to_idx.get(key.as_str()) {
                seen[idx] = true;
                let acc = &mut self.accumulators[idx];
                match val {
                    serde_json::Value::Null => acc.null_count += 1,
                    serde_json::Value::String(s) => acc.update(s, page_idx),
                    serde_json::Value::Number(n) => acc.update(&n.to_string(), page_idx),
                    serde_json::Value::Bool(b) => acc.update(if *b { "true" } else { "false" }, page_idx),
                    other => acc.update(&other.to_string(), page_idx),
                }
            }
        }

        for (idx, was_seen) in seen.into_iter().enumerate() {
            if !was_seen {
                self.accumulators[idx].null_count += 1;
            }
        }

        self.current_row += 1;
    }

    /// Finalize and return per-column statistics.
    pub fn finish(self) -> Vec<ColumnStats> {
        self.accumulators.into_iter().map(|a| a.finish()).collect()
    }
}
