//! Physical row group layout for line-oriented formats (CSV, JSON Lines).
//!
//! Breaks a file into page groups of ~5MB each. Each page records its
//! byte offset and starting row number. Given a logical row number,
//! binary search finds the containing page, then newline scanning
//! within the page locates the exact row.
//!
//! The layout file also stores per-column statistics captured at attach
//! time, avoiding re-scans for profiling and enabling query optimizations.
//!
//! # File Format (`prg.layout`)
//!
//! ```text
//! Magic:        "BBROWG01" (8 bytes)
//! Version:      u8 (1 byte)
//! Row count:    u64 little-endian (8 bytes)
//! File size:    u64 little-endian (8 bytes)
//! Page count:   u32 little-endian (4 bytes)
//! Stats offset: u64 little-endian (8 bytes)  — byte offset of stats blob
//! Pages:        [(physical_start: u64, row_begin: u32); page_count]  — 12 bytes each
//! Stats blob:   JSON-encoded Vec<ColumnStats> (positional: index = column order in file)
//! ```

use crate::layout_cache::GLOBAL_LAYOUT_CACHE;
use bundlebase_common::BundlebaseError;
use bundlebase_io::plugin::object_store::ObjectStoreFile;
use bundlebase_io::{IOReadFile, IOReadWriteDir};
use bytes::Bytes;
use futures::stream;
use futures::StreamExt;
use object_store::{GetOptions, GetRange, ObjectStore};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

const MAGIC_BYTES: &[u8; 8] = b"BBROWG01";
const VERSION: u8 = 1;
const HEADER_SIZE: usize = 8 + 1 + 8 + 8 + 4 + 8; // magic + version + row_count + file_size + page_count + stats_offset

/// Typed min/max/histogram-bound value stored in layout files.
///
/// Covers all orderable Arrow scalar types. Non-orderable types (List, Struct, Map)
/// are excluded — no statistics system computes min/max for them.
///
/// Serialized as a discriminated union: `{"t":"Int64","v":42}` so the type is preserved
/// across round-trips without implicit string parsing.
///
/// For CSV/JSONL columns: `Float64` when `is_all_numeric`, `Utf8` otherwise.
/// For Parquet columns: the native Arrow type from the schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
pub enum StatValue {
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    UInt64(u64),
    Float32(f32),
    Float64(f64),
    Utf8(String),
    Boolean(bool),
    /// Days since Unix epoch (Arrow Date32)
    Date32(i32),
    /// Milliseconds since Unix epoch (Arrow Date64)
    Date64(i64),
    TimestampSecond(i64),
    TimestampMillisecond(i64),
    TimestampMicrosecond(i64),
    TimestampNanosecond(i64),
    Time32Second(i32),
    Time32Millisecond(i32),
    Time64Microsecond(i64),
    Time64Nanosecond(i64),
}

impl StatValue {
    /// Compare this value against a query predicate `IndexedValue`.
    /// Returns `None` when the types are not in the same orderable family.
    ///
    /// Numeric types (Int* / UInt* / Float*) all compare via f64 — acceptable for
    /// pruning since false negatives only add scanned pages, never wrong results.
    /// Timestamp variants all compare via their raw i64 tick count.
    pub fn compare_to_indexed(
        &self,
        other: &bundlebase_index::IndexedValue,
    ) -> Option<std::cmp::Ordering> {
        use bundlebase_index::IndexedValue;
        match other {
            IndexedValue::Int64(q) => self.as_f64().and_then(|s| s.partial_cmp(&(*q as f64))),
            IndexedValue::Float64(q) => self.as_f64().and_then(|s| s.partial_cmp(&q.0)),
            IndexedValue::Utf8(q) => {
                if let StatValue::Utf8(sv) = self {
                    Some(sv.as_str().cmp(q.as_str()))
                } else {
                    None
                }
            }
            IndexedValue::Boolean(q) => {
                if let StatValue::Boolean(sv) = self {
                    Some(sv.cmp(q))
                } else {
                    None
                }
            }
            IndexedValue::Timestamp(q) => self.as_timestamp_i64().map(|s| s.cmp(q)),
            IndexedValue::Null => None,
        }
    }

    /// Convert numeric variants to f64 for cross-type comparison.
    fn as_f64(&self) -> Option<f64> {
        match self {
            StatValue::Int8(n) => Some(*n as f64),
            StatValue::Int16(n) => Some(*n as f64),
            StatValue::Int32(n) => Some(*n as f64),
            StatValue::Int64(n) => Some(*n as f64),
            StatValue::UInt8(n) => Some(*n as f64),
            StatValue::UInt16(n) => Some(*n as f64),
            StatValue::UInt32(n) => Some(*n as f64),
            StatValue::UInt64(n) => Some(*n as f64),
            StatValue::Float32(f) => Some(*f as f64),
            StatValue::Float64(f) => Some(*f),
            _ => None,
        }
    }

    /// Convert temporal variants to their raw i64 tick count for comparison
    /// against `IndexedValue::Timestamp` (nanoseconds since epoch).
    fn as_timestamp_i64(&self) -> Option<i64> {
        match self {
            StatValue::TimestampSecond(n)
            | StatValue::TimestampMillisecond(n)
            | StatValue::TimestampMicrosecond(n)
            | StatValue::TimestampNanosecond(n)
            | StatValue::Date64(n)
            | StatValue::Time64Microsecond(n)
            | StatValue::Time64Nanosecond(n)
            | StatValue::Int64(n) => Some(*n),
            StatValue::Date32(n) | StatValue::Time32Second(n) | StatValue::Time32Millisecond(n) => {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    /// Compare this value to another `StatValue` of the same type.
    /// Returns `None` if types differ or the comparison is not defined (e.g., NaN).
    pub fn cmp_to_stat(&self, other: &StatValue) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (StatValue::Int8(a), StatValue::Int8(b)) => Some(a.cmp(b)),
            (StatValue::Int16(a), StatValue::Int16(b)) => Some(a.cmp(b)),
            (StatValue::Int32(a), StatValue::Int32(b)) => Some(a.cmp(b)),
            (StatValue::Int64(a), StatValue::Int64(b)) => Some(a.cmp(b)),
            (StatValue::UInt8(a), StatValue::UInt8(b)) => Some(a.cmp(b)),
            (StatValue::UInt16(a), StatValue::UInt16(b)) => Some(a.cmp(b)),
            (StatValue::UInt32(a), StatValue::UInt32(b)) => Some(a.cmp(b)),
            (StatValue::UInt64(a), StatValue::UInt64(b)) => Some(a.cmp(b)),
            (StatValue::Float32(a), StatValue::Float32(b)) => a.partial_cmp(b),
            (StatValue::Float64(a), StatValue::Float64(b)) => a.partial_cmp(b),
            (StatValue::Utf8(a), StatValue::Utf8(b)) => Some(a.cmp(b)),
            (StatValue::Boolean(a), StatValue::Boolean(b)) => Some(a.cmp(b)),
            (StatValue::Date32(a), StatValue::Date32(b)) => Some(a.cmp(b)),
            (StatValue::Date64(a), StatValue::Date64(b)) => Some(a.cmp(b)),
            (StatValue::TimestampSecond(a), StatValue::TimestampSecond(b)) => Some(a.cmp(b)),
            (StatValue::TimestampMillisecond(a), StatValue::TimestampMillisecond(b)) => Some(a.cmp(b)),
            (StatValue::TimestampMicrosecond(a), StatValue::TimestampMicrosecond(b)) => Some(a.cmp(b)),
            (StatValue::TimestampNanosecond(a), StatValue::TimestampNanosecond(b)) => Some(a.cmp(b)),
            (StatValue::Time32Second(a), StatValue::Time32Second(b)) => Some(a.cmp(b)),
            (StatValue::Time32Millisecond(a), StatValue::Time32Millisecond(b)) => Some(a.cmp(b)),
            (StatValue::Time64Microsecond(a), StatValue::Time64Microsecond(b)) => Some(a.cmp(b)),
            (StatValue::Time64Nanosecond(a), StatValue::Time64Nanosecond(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }

    /// Human-readable string representation for display (e.g., DESCRIBE histogram output).
    pub fn display(&self) -> String {
        match self {
            StatValue::Int8(n) => n.to_string(),
            StatValue::Int16(n) => n.to_string(),
            StatValue::Int32(n) => n.to_string(),
            StatValue::Int64(n) => n.to_string(),
            StatValue::UInt8(n) => n.to_string(),
            StatValue::UInt16(n) => n.to_string(),
            StatValue::UInt32(n) => n.to_string(),
            StatValue::UInt64(n) => n.to_string(),
            StatValue::Float32(f) => f.to_string(),
            StatValue::Float64(f) => f.to_string(),
            StatValue::Utf8(s) => s.clone(),
            StatValue::Boolean(b) => b.to_string(),
            StatValue::Date32(n) => n.to_string(),
            StatValue::Date64(n) => n.to_string(),
            StatValue::TimestampSecond(n) => n.to_string(),
            StatValue::TimestampMillisecond(n) => n.to_string(),
            StatValue::TimestampMicrosecond(n) => n.to_string(),
            StatValue::TimestampNanosecond(n) => n.to_string(),
            StatValue::Time32Second(n) => n.to_string(),
            StatValue::Time32Millisecond(n) => n.to_string(),
            StatValue::Time64Microsecond(n) => n.to_string(),
            StatValue::Time64Nanosecond(n) => n.to_string(),
        }
    }
}

/// String distribution profile for a column (only present when the column is not all-numeric).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StringProfile {
    /// Minimum non-null value length in bytes.
    pub min_len: u32,
    /// Maximum non-null value length in bytes.
    pub max_len: u32,
    /// Average non-null value length in bytes.
    pub avg_len: f32,
    /// Fraction of non-null values that are pure ASCII (0.0–1.0).
    pub pct_ascii: f32,
}

/// One bucket in a value-distribution histogram.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistogramBucket {
    /// Inclusive lower bound of the bucket (typed value).
    /// For numeric columns the bucket represents an equal-width interval.
    /// For string columns it represents an equal-height (decile) interval.
    pub lower_bound: StatValue,
    /// Number of sampled values that fall in this bucket.
    pub count: u64,
}

/// Per-column statistics captured at attach time for CSV/JSONL blocks.
///
/// Statistics are indexed positionally — index N corresponds to column N in the file schema.
/// Stored in the layout file to avoid re-scanning the data for profiling or optimization.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ColumnStats {
    /// Number of null (empty) values in the column.
    pub null_count: u64,
    /// Minimum value seen (typed), or None if all values were null.
    pub min: Option<StatValue>,
    /// Maximum value seen (typed), or None if all values were null.
    pub max: Option<StatValue>,
    /// Top 10 most frequent non-null values with their counts.
    pub top_values: Vec<(String, u64)>,
    /// True if every non-null value in the column can be parsed as f64.
    pub is_all_numeric: bool,
    /// True if values are strictly increasing (numeric order if is_all_numeric, else lexicographic).
    pub is_strictly_increasing: bool,
    /// True if values are strictly decreasing (numeric order if is_all_numeric, else lexicographic).
    pub is_strictly_decreasing: bool,
    /// Approximate count of distinct non-null values. Up to 2000 tracked exactly; higher
    /// cardinality columns report the last known count before pruning.
    pub distinct_count: u64,
    /// Per-page statistics, positional: index i corresponds to layout.pages[i].
    pub page_stats: Vec<PageStats>,
    /// String distribution profile. None when is_all_numeric (string lengths of numbers
    /// are not meaningful) or when the column has no non-null values.
    pub string_profile: Option<StringProfile>,
    /// 10-bucket histogram of value distribution.
    /// Equal-width over [min, max] for numeric columns; equal-height (decile) for string columns.
    /// Built from a reservoir sample of up to 1000 values.
    pub histogram: Vec<HistogramBucket>,
}

/// Statistics for a single page of a column.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PageStats {
    /// Minimum non-null value seen in this page (typed), or None.
    pub min: Option<StatValue>,
    /// Maximum non-null value seen in this page (typed), or None.
    pub max: Option<StatValue>,
    /// Approximate distinct count for this page. Exact up to 500; capped at 500 for
    /// high-cardinality pages (true distinct count is ≥ 500 when this value is 500).
    pub distinct_count: u64,
    /// Serialized 8KB (65536-bit, 3-hash FNV-1a) bloom filter for per-page equality pruning.
    /// None when this page has ≥ 500 distinct values (FP rate rises above ~0.01% at that point).
    pub bloom_filter: Option<Vec<u8>>,
}

/// Default page size: 5MB
pub const DEFAULT_PAGE_SIZE: usize = 5 * 1024 * 1024;

/// A single page group entry: the byte offset where it starts and the
/// logical row number it begins at.
#[derive(Debug, Clone, PartialEq)]
pub struct PageGroup {
    /// Byte offset in the file where this page starts
    pub physical_start: u64,
    /// First logical row number in this page (0-indexed within the block)
    pub row_begin: u32,
}

/// Physical row group layout for a line-oriented file.
///
/// Maps logical row numbers to physical byte ranges by dividing the file
/// into pages of approximately `DEFAULT_PAGE_SIZE` bytes.
/// Also carries per-column statistics captured at attach time.
#[derive(Debug, Clone)]
pub struct PhysicalRowGroupLayout {
    /// Total number of data rows in the file
    pub total_rows: u64,
    /// Total file size in bytes
    pub file_size: u64,
    /// Page group entries, sorted by row_begin
    pub pages: Vec<PageGroup>,
    /// Per-column statistics, positional (index = column order in file schema).
    /// Empty when no stats were computed (e.g., layout loaded from storage with old format).
    pub column_stats: Vec<ColumnStats>,
}

impl PhysicalRowGroupLayout {
    /// Build a layout by scanning file bytes for newlines.
    ///
    /// Always returns `Some` — even small files get a layout so column stats are always stored.
    /// For files smaller than `page_size`, a single implicit page covers the whole file.
    ///
    /// # Arguments
    /// * `bytes` - The full file content
    /// * `skip_first_line` - Whether to skip the first line (e.g., CSV header)
    /// * `page_size` - Target page size in bytes (use `DEFAULT_PAGE_SIZE`)
    /// * `column_stats` - Pre-computed per-column statistics to embed in the layout
    pub fn build(
        bytes: &[u8],
        skip_first_line: bool,
        page_size: usize,
        column_stats: Vec<ColumnStats>,
    ) -> Option<PhysicalRowGroupLayout> {
        if bytes.is_empty() {
            return None;
        }

        // For small files, emit one page covering the whole file (pages are not needed
        // for row lookup but we still write the layout for column stats).
        let use_pages = bytes.len() > page_size;

        let mut pages = Vec::new();
        let mut row_count: u32 = 0;
        let mut page_start: u64 = 0;
        let mut skip_first = skip_first_line;

        // Start first page
        pages.push(PageGroup {
            physical_start: 0,
            row_begin: 0,
        });

        for (i, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                if skip_first {
                    skip_first = false;
                    // Adjust page start past the header
                    page_start = (i + 1) as u64;
                    pages[0].physical_start = page_start;
                } else {
                    row_count += 1;
                }

                if use_pages {
                    let page_bytes = (i as u64 + 1 - page_start) as usize;

                    // Start new page if current page exceeds target size
                    if page_bytes >= page_size && !skip_first {
                        let new_page_start = (i + 1) as u64;
                        page_start = new_page_start;

                        pages.push(PageGroup {
                            physical_start: new_page_start,
                            row_begin: row_count,
                        });
                    }
                }
            }
        }

        // Handle final line without newline
        if !bytes.is_empty() && bytes[bytes.len() - 1] != b'\n' && !skip_first {
            row_count += 1;
        }

        if row_count == 0 {
            return None;
        }

        Some(PhysicalRowGroupLayout {
            total_rows: row_count as u64,
            file_size: bytes.len() as u64,
            pages,
            column_stats,
        })
    }

    /// Build a layout from a file and write it to content-addressed storage.
    ///
    /// Always writes a layout file (even for small files) so column stats are always stored.
    /// Returns None only if the file is empty or has no data rows.
    ///
    /// # Arguments
    /// * `datafile` - The data file to build a layout for
    /// * `data_dir` - Directory to write the layout file into
    /// * `skip_first_line` - Whether to skip the first line (e.g., CSV header)
    /// * `column_stats` - Pre-computed per-column statistics to embed
    pub async fn build_and_write(
        datafile: &ObjectStoreFile,
        data_dir: &dyn IOReadWriteDir,
        skip_first_line: bool,
        column_stats: Vec<ColumnStats>,
    ) -> Result<Option<Box<dyn bundlebase_io::IOReadFile>>, BundlebaseError> {
        // Read the full file
        let mut file_stream = datafile.read_existing().await?;
        let mut buffer = Vec::new();
        while let Some(chunk_result) = file_stream.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }

        match Self::build(&buffer, skip_first_line, DEFAULT_PAGE_SIZE, column_stats) {
            None => Ok(None),
            Some(layout) => {
                let index_bytes = layout.serialize()?;
                let data_stream =
                    Box::pin(stream::once(async { Ok::<_, std::io::Error>(index_bytes) }));
                let address = bundlebase_common::ContentAddress::with_sub_type(
                    bundlebase_common::ContentCategory::Block,
                    "layout",
                    bundlebase_common::ContentFormat::Pagemap,
                )?;
                let result = data_dir.write_stream(data_stream, &address).await?;
                Ok(Some(result.file))
            }
        }
    }

    /// Serialize to binary format.
    pub fn serialize(&self) -> Result<Bytes, BundlebaseError> {
        let page_count = self.pages.len();
        let pages_size = page_count * 12;
        // stats_offset points to the byte immediately after the page entries
        let stats_offset = (HEADER_SIZE + pages_size) as u64;
        let stats_json = serde_json::to_vec(&self.column_stats)
            .map_err(|e| BundlebaseError::from(format!("Failed to serialize column stats: {}", e)))?;

        let mut buffer = Vec::with_capacity(HEADER_SIZE + pages_size + stats_json.len());

        buffer.extend_from_slice(MAGIC_BYTES);
        buffer.push(VERSION);
        buffer.extend_from_slice(&self.total_rows.to_le_bytes());
        buffer.extend_from_slice(&self.file_size.to_le_bytes());
        buffer.extend_from_slice(&(page_count as u32).to_le_bytes());
        buffer.extend_from_slice(&stats_offset.to_le_bytes());

        for page in &self.pages {
            buffer.extend_from_slice(&page.physical_start.to_le_bytes());
            buffer.extend_from_slice(&page.row_begin.to_le_bytes());
        }

        buffer.extend_from_slice(&stats_json);

        Ok(Bytes::from(buffer))
    }

    /// Deserialize from binary format.
    pub fn deserialize(bytes: &[u8]) -> Result<PhysicalRowGroupLayout, BundlebaseError> {
        if bytes.len() < HEADER_SIZE {
            return Err("Invalid layout file: too short".into());
        }

        if &bytes[0..8] != MAGIC_BYTES {
            return Err("Invalid layout file: bad magic bytes".into());
        }

        let version = bytes[8];
        if version != VERSION {
            return Err(format!("Unsupported layout version: {}", version).into());
        }

        let total_rows = u64::from_le_bytes(bytes[9..17].try_into().map_err(|_| {
            BundlebaseError::from("Invalid layout file: bad total_rows")
        })?);

        let file_size = u64::from_le_bytes(bytes[17..25].try_into().map_err(|_| {
            BundlebaseError::from("Invalid layout file: bad file_size")
        })?);

        let page_count = u32::from_le_bytes(bytes[25..29].try_into().map_err(|_| {
            BundlebaseError::from("Invalid layout file: bad page_count")
        })?) as usize;

        let stats_offset = u64::from_le_bytes(bytes[29..37].try_into().map_err(|_| {
            BundlebaseError::from("Invalid layout file: bad stats_offset")
        })?) as usize;

        let pages_end = HEADER_SIZE + page_count * 12;
        if bytes.len() < pages_end {
            return Err(format!(
                "Invalid layout file: expected {} bytes, got {}",
                pages_end,
                bytes.len()
            )
            .into());
        }

        let mut pages = Vec::with_capacity(page_count);
        for i in 0..page_count {
            let offset = HEADER_SIZE + i * 12;
            let physical_start = u64::from_le_bytes(
                bytes[offset..offset + 8]
                    .try_into()
                    .map_err(|_| BundlebaseError::from("Invalid layout: bad physical_start"))?,
            );
            let row_begin = u32::from_le_bytes(
                bytes[offset + 8..offset + 12]
                    .try_into()
                    .map_err(|_| BundlebaseError::from("Invalid layout: bad row_begin"))?,
            );
            pages.push(PageGroup {
                physical_start,
                row_begin,
            });
        }

        // Parse column stats from the JSON blob if present
        let column_stats = if stats_offset < bytes.len() {
            serde_json::from_slice(&bytes[stats_offset..]).unwrap_or_default()
        } else {
            Vec::new()
        };

        Ok(PhysicalRowGroupLayout {
            total_rows,
            file_size,
            pages,
            column_stats,
        })
    }

    /// Load a layout from a file.
    pub async fn load(file: &ObjectStoreFile) -> Result<PhysicalRowGroupLayout, BundlebaseError> {
        let mut stream = file.read_existing().await?;
        let mut buffer = Vec::new();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            buffer.extend_from_slice(&chunk);
        }
        Self::deserialize(&buffer)
    }

    /// Find which page contains the given logical row number.
    /// Returns the page index, or None if the row is out of range.
    pub fn find_page(&self, row_number: u32) -> Option<usize> {
        if row_number as u64 >= self.total_rows {
            return None;
        }
        // Binary search: find the last page where row_begin <= row_number
        match self.pages.binary_search_by_key(&row_number, |p| p.row_begin) {
            Ok(idx) => Some(idx),
            Err(idx) => {
                if idx == 0 {
                    None
                } else {
                    Some(idx - 1)
                }
            }
        }
    }

    /// Get the byte range for a page (start..end).
    /// The end is derived from the next page's start, or file_size for the last page.
    pub fn page_byte_range(&self, page_idx: usize) -> (u64, u64) {
        let start = self.pages[page_idx].physical_start;
        let end = if page_idx + 1 < self.pages.len() {
            self.pages[page_idx + 1].physical_start
        } else {
            self.file_size
        };
        (start, end)
    }

    /// Get the row range for a page (begin..end).
    /// The end is derived from the next page's row_begin, or total_rows for the last page.
    pub fn page_row_range(&self, page_idx: usize) -> (u32, u32) {
        let begin = self.pages[page_idx].row_begin;
        let end = if page_idx + 1 < self.pages.len() {
            self.pages[page_idx + 1].row_begin
        } else {
            self.total_rows as u32
        };
        (begin, end)
    }
}

/// Resolve logical row numbers to physical byte offsets in a line-oriented file.
///
/// Uses the layout file (if available and cached) to minimize I/O by reading
/// only the relevant pages. For small files without a layout, reads the full file.
///
/// Returns byte offsets corresponding to each input row number, sorted.
pub async fn resolve_row_numbers_to_byte_offsets(
    data_file: &ObjectStoreFile,
    layout_file: Option<&ObjectStoreFile>,
    row_numbers: &[u32],
    skip_header: bool,
) -> Result<Vec<u64>, BundlebaseError> {
    if row_numbers.is_empty() {
        return Ok(Vec::new());
    }

    let store = data_file.store();
    let path = data_file.store_path().clone();

    // Try to load layout (from cache or disk)
    let layout = match layout_file {
        Some(lf) => {
            let url = lf.url().clone();
            if let Some(cached) = GLOBAL_LAYOUT_CACHE.get(&url) {
                log::trace!("Layout cache hit for {}", url);
                Some(cached)
            } else {
                log::debug!("Layout cache miss for {}, loading from disk", url);
                let loaded = PhysicalRowGroupLayout::load(lf).await?;
                let arc = Arc::new(loaded);
                GLOBAL_LAYOUT_CACHE.insert(url, arc.clone());
                Some(arc)
            }
        }
        None => None,
    };

    match layout {
        Some(layout) => resolve_with_layout(&store, &path, &layout, row_numbers).await,
        None => resolve_without_layout(&store, &path, row_numbers, skip_header).await,
    }
}

/// Resolve row numbers using a layout to read only the relevant pages.
async fn resolve_with_layout(
    store: &Arc<dyn ObjectStore>,
    path: &object_store::path::Path,
    layout: &PhysicalRowGroupLayout,
    row_numbers: &[u32],
) -> Result<Vec<u64>, BundlebaseError> {
    // Group row numbers by page
    let mut pages_to_rows: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
    for &row_num in row_numbers {
        if let Some(page_idx) = layout.find_page(row_num) {
            pages_to_rows.entry(page_idx).or_default().push(row_num);
        } else {
            return Err(format!(
                "Row number {} out of range (file has {} rows)",
                row_num, layout.total_rows
            )
            .into());
        }
    }

    let mut result: Vec<u64> = Vec::with_capacity(row_numbers.len());

    for (page_idx, mut target_rows) in pages_to_rows {
        target_rows.sort();
        let (page_start, page_end) = layout.page_byte_range(page_idx);
        let page_row_begin = layout.pages[page_idx].row_begin;

        // Read page bytes
        let range = GetRange::Bounded(page_start..page_end);
        let options = GetOptions {
            range: Some(range),
            ..Default::default()
        };
        let get_result = store.get_opts(path, options).await?;
        let bytes = get_result.bytes().await?;

        // Scan newlines to build row-offset map within this page
        let offsets = scan_newline_offsets(&bytes, page_start, page_row_begin, &target_rows);
        result.extend(offsets);
    }

    result.sort();
    Ok(result)
}

/// Resolve row numbers by reading the entire file (for small files without a layout).
async fn resolve_without_layout(
    store: &Arc<dyn ObjectStore>,
    path: &object_store::path::Path,
    row_numbers: &[u32],
    skip_header: bool,
) -> Result<Vec<u64>, BundlebaseError> {
    let get_result = store.get_opts(path, GetOptions::default()).await?;
    let bytes = get_result.bytes().await?;

    let mut sorted_targets: Vec<u32> = row_numbers.to_vec();
    sorted_targets.sort();

    // Find the data start (skip header if needed)
    let data_start = if skip_header {
        match bytes.iter().position(|&b| b == b'\n') {
            Some(pos) => (pos + 1) as u64,
            None => return Err("File has no newlines but skip_header was requested".into()),
        }
    } else {
        0
    };

    let offsets = scan_newline_offsets(&bytes[data_start as usize..], data_start, 0, &sorted_targets);
    Ok(offsets)
}

/// Scan bytes for newlines and return byte offsets for the requested row numbers.
///
/// `bytes` is the content to scan.
/// `base_offset` is the absolute byte offset of `bytes[0]` in the file.
/// `base_row` is the logical row number of the first row in `bytes`.
/// `target_rows` must be sorted and all >= `base_row`.
fn scan_newline_offsets(
    bytes: &[u8],
    base_offset: u64,
    base_row: u32,
    target_rows: &[u32],
) -> Vec<u64> {
    if target_rows.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(target_rows.len());
    let mut current_row = base_row;
    let mut target_idx = 0;
    let mut line_start: u64 = base_offset;

    // If the first target is the first row in this range, emit its offset immediately
    if target_rows[target_idx] == current_row {
        result.push(line_start);
        target_idx += 1;
        if target_idx >= target_rows.len() {
            return result;
        }
    }

    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            current_row += 1;
            line_start = base_offset + (i as u64) + 1;

            if target_idx < target_rows.len() && target_rows[target_idx] == current_row {
                result.push(line_start);
                target_idx += 1;
                if target_idx >= target_rows.len() {
                    break;
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_empty_file() {
        let result = PhysicalRowGroupLayout::build(b"", false, DEFAULT_PAGE_SIZE, vec![]);
        assert!(result.is_none());
    }

    #[test]
    fn test_build_file_smaller_than_page_size() {
        // Small files still get a layout (for column stats), just with one page.
        let data = b"row1\nrow2\n";
        let result = PhysicalRowGroupLayout::build(data, false, DEFAULT_PAGE_SIZE, vec![]).unwrap();
        assert_eq!(result.total_rows, 2);
        assert_eq!(result.pages.len(), 1, "Single page for small files");
    }

    #[test]
    fn test_build_with_small_page_size() {
        // Use a tiny page size to test page splitting
        let data = b"row1\nrow2\nrow3\nrow4\nrow5\nrow6\n";
        let result = PhysicalRowGroupLayout::build(data, false, 10, vec![]).unwrap();

        assert_eq!(result.total_rows, 6);
        assert_eq!(result.file_size, data.len() as u64);
        assert!(result.pages.len() >= 2, "Should have multiple pages with 10-byte page size");

        // First page always starts at 0
        assert_eq!(result.pages[0].physical_start, 0);
        assert_eq!(result.pages[0].row_begin, 0);
    }

    #[test]
    fn test_build_with_header_skip() {
        let data = b"header\nrow1\nrow2\nrow3\n";
        let result = PhysicalRowGroupLayout::build(data, true, 10, vec![]).unwrap();

        // Should have 3 data rows (header skipped)
        assert_eq!(result.total_rows, 3);
        // First page should start after the header
        assert_eq!(result.pages[0].physical_start, 7); // "header\n" is 7 bytes
    }

    #[test]
    fn test_build_header_only() {
        let data = b"header\n";
        let result = PhysicalRowGroupLayout::build(data, true, DEFAULT_PAGE_SIZE, vec![]);
        assert!(result.is_none(), "No rows after skipping header");
    }

    #[test]
    fn test_build_no_trailing_newline() {
        let data = b"row1\nrow2\nrow3";
        // Use small page to force layout creation
        let result = PhysicalRowGroupLayout::build(data, false, 5, vec![]).unwrap();
        assert_eq!(result.total_rows, 3); // row3 counted even without \n
    }

    #[test]
    fn test_build_single_large_row() {
        // One row larger than page size
        let data = vec![b'x'; 20];
        let mut data_with_newline = data.clone();
        data_with_newline.push(b'\n');
        // Add another row to make file bigger than page size
        data_with_newline.extend_from_slice(&vec![b'y'; 20]);
        data_with_newline.push(b'\n');

        let result = PhysicalRowGroupLayout::build(&data_with_newline, false, 10, vec![]).unwrap();
        assert_eq!(result.total_rows, 2);
        // Large row gets its own page
        assert!(result.pages.len() >= 1);
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let stats = ColumnStats {
            null_count: 5,
            min: Some(StatValue::Float64(1.0)),
            max: Some(StatValue::Float64(99.0)),
            top_values: vec![("foo".to_string(), 10), ("bar".to_string(), 7)],
            is_all_numeric: true,
            is_strictly_increasing: false,
            is_strictly_decreasing: false,
            distinct_count: 42,
            page_stats: vec![
                PageStats { min: Some(StatValue::Float64(1.0)), max: Some(StatValue::Float64(50.0)), distinct_count: 20, bloom_filter: None },
                PageStats { min: Some(StatValue::Float64(51.0)), max: Some(StatValue::Float64(99.0)), distinct_count: 22, bloom_filter: None },
            ],
            ..Default::default()
        };
        let layout = PhysicalRowGroupLayout {
            total_rows: 100,
            file_size: 50000,
            pages: vec![
                PageGroup { physical_start: 0, row_begin: 0 },
                PageGroup { physical_start: 25000, row_begin: 50 },
            ],
            column_stats: vec![stats],
        };

        let bytes = layout.serialize().expect("serialize should succeed");
        let loaded = PhysicalRowGroupLayout::deserialize(&bytes).unwrap();

        assert_eq!(loaded.total_rows, 100);
        assert_eq!(loaded.file_size, 50000);
        assert_eq!(loaded.pages.len(), 2);
        assert_eq!(loaded.pages[0], layout.pages[0]);
        assert_eq!(loaded.pages[1], layout.pages[1]);
        assert_eq!(loaded.column_stats.len(), 1);
        assert_eq!(loaded.column_stats[0].null_count, 5);
        assert_eq!(loaded.column_stats[0].min, Some(StatValue::Float64(1.0)));
        assert_eq!(loaded.column_stats[0].top_values.len(), 2);
        assert!(loaded.column_stats[0].is_all_numeric);
        assert_eq!(loaded.column_stats[0].distinct_count, 42);
        assert_eq!(loaded.column_stats[0].page_stats.len(), 2);
        assert_eq!(loaded.column_stats[0].page_stats[0].min, Some(StatValue::Float64(1.0)));
        assert_eq!(loaded.column_stats[0].page_stats[0].max, Some(StatValue::Float64(50.0)));
        assert_eq!(loaded.column_stats[0].page_stats[1].min, Some(StatValue::Float64(51.0)));
        assert_eq!(loaded.column_stats[0].page_stats[1].max, Some(StatValue::Float64(99.0)));
    }

    #[test]
    fn test_deserialize_bad_magic() {
        // 37 bytes minimum for new header (old was 29, new adds 8 for stats_offset)
        let bytes = b"WRONGMAG\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        let result = PhysicalRowGroupLayout::deserialize(bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_too_short() {
        let result = PhysicalRowGroupLayout::deserialize(&[0u8; 10]);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_page() {
        let layout = PhysicalRowGroupLayout {
            total_rows: 300,
            file_size: 150000,
            pages: vec![
                PageGroup { physical_start: 0, row_begin: 0 },
                PageGroup { physical_start: 50000, row_begin: 100 },
                PageGroup { physical_start: 100000, row_begin: 200 },
            ],
            column_stats: vec![],
        };

        assert_eq!(layout.find_page(0), Some(0));
        assert_eq!(layout.find_page(50), Some(0));
        assert_eq!(layout.find_page(99), Some(0));
        assert_eq!(layout.find_page(100), Some(1));
        assert_eq!(layout.find_page(199), Some(1));
        assert_eq!(layout.find_page(200), Some(2));
        assert_eq!(layout.find_page(299), Some(2));
        assert_eq!(layout.find_page(300), None); // out of range
    }

    #[test]
    fn test_page_byte_range() {
        let layout = PhysicalRowGroupLayout {
            total_rows: 300,
            file_size: 150000,
            pages: vec![
                PageGroup { physical_start: 0, row_begin: 0 },
                PageGroup { physical_start: 50000, row_begin: 100 },
                PageGroup { physical_start: 100000, row_begin: 200 },
            ],
            column_stats: vec![],
        };

        assert_eq!(layout.page_byte_range(0), (0, 50000));
        assert_eq!(layout.page_byte_range(1), (50000, 100000));
        assert_eq!(layout.page_byte_range(2), (100000, 150000)); // last page ends at file_size
    }

    #[test]
    fn test_page_row_range() {
        let layout = PhysicalRowGroupLayout {
            total_rows: 300,
            file_size: 150000,
            pages: vec![
                PageGroup { physical_start: 0, row_begin: 0 },
                PageGroup { physical_start: 50000, row_begin: 100 },
                PageGroup { physical_start: 100000, row_begin: 200 },
            ],
            column_stats: vec![],
        };

        assert_eq!(layout.page_row_range(0), (0, 100));
        assert_eq!(layout.page_row_range(1), (100, 200));
        assert_eq!(layout.page_row_range(2), (200, 300));
    }

    #[test]
    fn test_build_and_find_consistency() {
        // Build a layout and verify find_page works for all rows
        let mut data = Vec::new();
        for i in 0..100 {
            data.extend_from_slice(format!("row_{:04}\n", i).as_bytes());
        }

        // Use 50-byte page size to get multiple pages
        let layout = PhysicalRowGroupLayout::build(&data, false, 50, vec![]).unwrap();
        assert_eq!(layout.total_rows, 100);

        // Every row should map to a valid page
        for row in 0..100u32 {
            let page_idx = layout.find_page(row);
            assert!(page_idx.is_some(), "Row {} should have a page", row);

            let (row_begin, row_end) = layout.page_row_range(page_idx.unwrap());
            assert!(row >= row_begin && row < row_end,
                "Row {} should be in page range [{}, {})", row, row_begin, row_end);
        }
    }

    #[test]
    fn test_scan_newline_offsets_basic() {
        // "row0\nrow1\nrow2\n"
        let data = b"row0\nrow1\nrow2\n";
        let offsets = scan_newline_offsets(data, 0, 0, &[0, 1, 2]);
        assert_eq!(offsets, vec![0, 5, 10]);
    }

    #[test]
    fn test_scan_newline_offsets_subset() {
        let data = b"row0\nrow1\nrow2\nrow3\n";
        let offsets = scan_newline_offsets(data, 0, 0, &[1, 3]);
        assert_eq!(offsets, vec![5, 15]);
    }

    #[test]
    fn test_scan_newline_offsets_with_base() {
        // Simulate reading a page starting at byte 100, row 10
        let data = b"row10\nrow11\nrow12\n";
        let offsets = scan_newline_offsets(data, 100, 10, &[10, 12]);
        assert_eq!(offsets, vec![100, 112]);
    }

    #[test]
    fn test_scan_newline_offsets_empty_targets() {
        let data = b"row0\nrow1\n";
        let offsets = scan_newline_offsets(data, 0, 0, &[]);
        assert!(offsets.is_empty());
    }
}
