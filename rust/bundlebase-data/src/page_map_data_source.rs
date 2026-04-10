use bundlebase_io::plugin::object_store::ObjectStoreFile;
use bundlebase_io::IOReadFile;
use arrow::csv::ReaderBuilder as CsvReaderBuilder;
use arrow::datatypes::SchemaRef;
use arrow::json::ReaderBuilder as JsonReaderBuilder;
use arrow::record_batch::RecordBatch;
use datafusion::common::{project_schema, DataFusionError, Statistics};
use datafusion::datasource::source::DataSource;
use datafusion::execution::TaskContext;
use datafusion::physical_expr::EquivalenceProperties;
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{DisplayFormatType, Partitioning, SendableRecordBatchStream};
use futures::stream::{self, StreamExt};
use object_store::{GetOptions, GetRange, ObjectStore};
use std::any::Any;
use std::fmt::{Debug, Display, Formatter};
use std::io::Cursor;
use std::sync::Arc;

/// File format for line-oriented data sources
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOrientedFormat {
    /// CSV format with header
    Csv,
    /// JSON Lines format (newline-delimited JSON)
    JsonLines,
}

/// Custom DataSource that reads only specified rows or page ranges from line-oriented files.
///
/// Two modes:
/// - **Row-offset mode** (for index-based reads): reads individual rows by byte offset with 4KB
///   read-ahead, coalescing nearby offsets into single range requests.
/// - **Page-range mode** (for page-filtered reads): reads contiguous byte ranges (one or more
///   coalesced pages) and parses all lines within each range.
///
/// Supports both CSV and JSON Lines formats.
pub struct PageMapDataSource {
    /// The source file
    file: ObjectStoreFile,
    /// Schema of the data (original full schema)
    schema: SchemaRef,
    /// Schema after projection is applied (computed at construction time)
    projected_schema: SchemaRef,
    /// Row-offset mode: byte positions of individual rows to read (sorted for sequential reading).
    /// Empty when in page-range mode.
    byte_offsets: Vec<u64>,
    /// Page-range mode: coalesced byte ranges `(inclusive_start, exclusive_end)` to read in full.
    /// Empty when in row-offset mode.
    page_ranges: Vec<(u64, u64)>,
    /// Number of rows/ranges (for statistics)
    num_rows: usize,
    /// Optional column projection (indices of columns to read)
    projection: Option<Vec<usize>>,
    /// Object store for reading file data
    object_store: Arc<dyn ObjectStore>,
    /// File format (CSV or JSON Lines)
    format: LineOrientedFormat,
}

impl PageMapDataSource {
    /// Create a PageMapDataSource from byte offsets.
    ///
    /// # Arguments
    /// * `file` - The source file
    /// * `schema` - Schema of the data
    /// * `byte_offsets` - Byte positions of rows to read
    /// * `projection` - Optional column projection
    /// * `format` - File format (CSV or JSON Lines)
    pub fn new(
        file: &ObjectStoreFile,
        schema: SchemaRef,
        byte_offsets: Vec<u64>,
        projection: Option<Vec<usize>>,
        format: LineOrientedFormat,
    ) -> Self {
        let mut sorted_offsets = byte_offsets;
        sorted_offsets.sort();
        let num_rows = sorted_offsets.len();

        let object_store = file.store();
        let projected_schema =
            project_schema(&schema, projection.as_ref()).expect("Failed to project schema");

        Self {
            file: file.clone(),
            schema,
            projected_schema,
            byte_offsets: sorted_offsets,
            page_ranges: Vec::new(),
            num_rows,
            projection,
            object_store,
            format,
        }
    }

    /// Create a PageMapDataSource from coalesced page byte ranges.
    ///
    /// Each range `(start, end)` is a contiguous byte span in the file. All lines within each
    /// range are parsed and returned. Adjacent or nearby pages should be merged by the caller
    /// before calling this constructor (see [`coalesce_page_ranges`]).
    ///
    /// # Arguments
    /// * `file` - The source file
    /// * `schema` - Schema of the data
    /// * `page_ranges` - Byte ranges `(inclusive_start, exclusive_end)` to read
    /// * `projection` - Optional column projection
    /// * `format` - File format (CSV or JSON Lines)
    pub fn from_page_ranges(
        file: &ObjectStoreFile,
        schema: SchemaRef,
        page_ranges: Vec<(u64, u64)>,
        projection: Option<Vec<usize>>,
        format: LineOrientedFormat,
    ) -> Self {
        let num_rows = page_ranges.len(); // approximate; actual row count unknown until read
        let object_store = file.store();
        let projected_schema =
            project_schema(&schema, projection.as_ref()).expect("Failed to project schema");

        Self {
            file: file.clone(),
            schema,
            projected_schema,
            byte_offsets: Vec::new(),
            page_ranges,
            num_rows,
            projection,
            object_store,
            format,
        }
    }

    /// Extract lines at specific byte offsets from a fetched byte range
    /// Works for both CSV and JSON Lines since both are line-oriented
    /// `batch_start` is the absolute byte offset of the start of `bytes` in the file
    /// `row_offsets` are absolute byte offsets of individual rows in the file
    /// Lines truncated by the read-ahead buffer (no newline found) are skipped
    fn extract_lines(bytes: &[u8], batch_start: u64, row_offsets: &[u64]) -> Vec<String> {
        if bytes.is_empty() || row_offsets.is_empty() {
            return Vec::new();
        }

        let text = String::from_utf8_lossy(bytes);
        let mut lines = Vec::new();

        for &offset in row_offsets {
            let pos = (offset - batch_start) as usize;
            if pos >= text.len() {
                continue; // offset beyond buffer
            }
            if let Some(end) = text[pos..].find('\n') {
                let trimmed = text[pos..pos + end].trim();
                if !trimmed.is_empty() {
                    lines.push(trimmed.to_string());
                }
            }
            // No newline → line truncated by read-ahead buffer, skip
        }

        lines
    }

    /// Read-ahead per row for line-oriented index reads.
    /// Rows in CSV/JSON are typically < 1KB. 4KB provides ample buffer
    /// while reading ~250x less data than the previous 1MB minimum.
    const LINE_READ_AHEAD_BYTES: u64 = 4096;

    /// Group byte offsets into batches for efficient fetching.
    /// Offsets that are close together (within 4KB) are batched into a single read.
    fn batch_offsets(offsets: &[u64]) -> Vec<(u64, u64, Vec<u64>)> {
        if offsets.is_empty() {
            return Vec::new();
        }

        let mut batches = Vec::new();
        let mut current_start = offsets[0];
        let mut current_end = current_start + Self::LINE_READ_AHEAD_BYTES;
        let mut current_offsets = vec![offsets[0]];

        for &offset in &offsets[1..] {
            let row_start = offset;
            let row_end = row_start + Self::LINE_READ_AHEAD_BYTES;

            // If this row starts within or near the current batch range, expand the batch
            if row_start <= current_end {
                // Expand the end if this row extends beyond current end
                current_end = current_end.max(row_end);
                current_offsets.push(row_start);
            } else {
                // Start a new batch - use mem::take to avoid cloning
                batches.push((current_start, current_end, std::mem::take(&mut current_offsets)));
                current_start = row_start;
                current_end = row_end;
                current_offsets.push(row_start);
            }
        }

        // Push the last batch
        batches.push((current_start, current_end, current_offsets));

        batches
    }
}

impl Debug for PageMapDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageMapDataSource")
            .field("file", &self.file)
            .field("schema", &self.schema)
            .field("num_offsets", &self.byte_offsets.len())
            .field("projection", &self.projection)
            .field("format", &self.format)
            .finish()
    }
}

impl Display for PageMapDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PageMapDataSource[file={}, rows={}, format={:?}]",
            self.file.url(),
            self.num_rows,
            self.format
        )
    }
}

impl DataSource for PageMapDataSource {
    fn open(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        let schema = self.schema.clone();
        let output_schema = self.projected_schema.clone();
        let object_store = self.object_store.clone();
        let file_path = self.file.store_path().clone();
        let projection = self.projection.clone();
        let format = self.format;

        if !self.page_ranges.is_empty() {
            // Page-range mode: read full page byte ranges, parse all lines in each range.
            // Used for page-filtered reads (avoids reading entire file when only some pages match).
            let page_ranges = self.page_ranges.clone();
            log::debug!(
                "PageMapDataSource: page-range mode, {} ranges",
                page_ranges.len()
            );

            let stream = stream::iter(page_ranges).then(move |(range_start, range_end)| {
                let object_store = object_store.clone();
                let file_path = file_path.clone();
                let schema = schema.clone();
                let projection = projection.clone();

                async move {
                    // Fetch the full page range in one ObjectStore call
                    let range = GetRange::Bounded(range_start..range_end);
                    let options = GetOptions { range: Some(range), ..Default::default() };

                    let bytes = match object_store.get_opts(&file_path, options).await {
                        Ok(r) => r.bytes().await.map_err(|e| DataFusionError::External(Box::new(e)))?,
                        Err(e) => return Err(DataFusionError::External(Box::new(e))),
                    };

                    if bytes.is_empty() {
                        return Ok(RecordBatch::new_empty(
                            project_schema(&schema, projection.as_ref())
                                ?,
                        ));
                    }

                    // Parse all lines in the range; format dictates whether to add a header.
                    parse_bytes_to_batch(&bytes, &schema, &projection, format)
                }
            });

            return Ok(Box::pin(RecordBatchStreamAdapter::new(output_schema, stream)));
        }

        // Row-offset mode: read individual rows by byte offset with 4KB read-ahead.
        // Used for index-based query optimization.
        let byte_offsets = self.byte_offsets.clone();
        let batches = Self::batch_offsets(&byte_offsets);

        log::debug!(
            "PageMapDataSource: row-offset mode, {} offsets → {} batches",
            byte_offsets.len(),
            batches.len()
        );

        let stream = stream::iter(batches).then(move |(batch_start, batch_end, batch_offsets)| {
            let object_store = object_store.clone();
            let file_path = file_path.clone();
            let schema = schema.clone();
            let projection = projection.clone();

            async move {
                let range = GetRange::Bounded(batch_start..batch_end);
                let options = GetOptions { range: Some(range), ..Default::default() };

                let bytes = match object_store.get_opts(&file_path, options).await {
                    Ok(r) => r.bytes().await.map_err(|e| DataFusionError::External(Box::new(e)))?,
                    Err(e) => return Err(DataFusionError::External(Box::new(e))),
                };

                let lines = Self::extract_lines(&bytes, batch_start, &batch_offsets);

                if lines.is_empty() {
                    return Ok(RecordBatch::new_empty(
                        project_schema(&schema, projection.as_ref())
                            ?,
                    ));
                }

                let batch = match format {
                    LineOrientedFormat::Csv => {
                        let lines_len: usize = lines.iter().map(|l| l.len() + 1).sum();
                        let header_len: usize = schema.fields().iter().map(|f| f.name().len() + 1).sum();
                        let mut csv_data = String::with_capacity(header_len + lines_len);
                        let mut first = true;
                        for field in schema.fields() {
                            if !first { csv_data.push(','); }
                            csv_data.push_str(field.name());
                            first = false;
                        }
                        csv_data.push('\n');
                        for line in lines { csv_data.push_str(&line); csv_data.push('\n'); }
                        let cursor = Cursor::new(csv_data.as_bytes());
                        let mut reader = CsvReaderBuilder::new(schema.clone())
                            .with_header(true).build(cursor)
                            ?;
                        reader.next()
                            .ok_or_else(|| DataFusionError::Internal("No batch produced".to_string()))?
                            ?
                    }
                    LineOrientedFormat::JsonLines => {
                        let total_len: usize = lines.iter().map(|l| l.len() + 1).sum();
                        let mut json_data = String::with_capacity(total_len);
                        for line in lines { json_data.push_str(&line); json_data.push('\n'); }
                        let cursor = Cursor::new(json_data.as_bytes());
                        let mut reader = JsonReaderBuilder::new(schema.clone())
                            .build(cursor)
                            ?;
                        reader.next()
                            .ok_or_else(|| DataFusionError::Internal("No batch produced".to_string()))?
                            ?
                    }
                };

                // Apply projection
                if let Some(proj) = &projection {
                    let projected_columns: Vec<_> =
                        proj.iter().map(|&i| batch.column(i).clone()).collect();
                    let projected_schema = Arc::new(
                        schema.project(proj)?,
                    );
                    RecordBatch::try_new(projected_schema, projected_columns)
                        .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))
                } else {
                    Ok(batch)
                }
            }
        });

        Ok(Box::pin(RecordBatchStreamAdapter::new(output_schema, stream)))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: DisplayFormatType, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "{}", self)
    }

    fn output_partitioning(&self) -> Partitioning {
        Partitioning::UnknownPartitioning(1)
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        // Return projected schema, not original schema
        // This ensures DataFusion knows what schema the execution plan will actually produce
        EquivalenceProperties::new(self.projected_schema.clone())
    }

    fn partition_statistics(
        &self,
        _partition: Option<usize>,
    ) -> datafusion::common::Result<Statistics> {
        // Return statistics based on the row IDs we'll read
        let mut stats = Statistics::new_unknown(&self.schema);
        stats.num_rows = datafusion::common::stats::Precision::Exact(self.num_rows);
        Ok(stats)
    }

    fn with_fetch(&self, _limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        // TODO: Implement fetch limit support
        None
    }

    fn fetch(&self) -> Option<usize> {
        None
    }

    fn try_swapping_with_projection(
        &self,
        _projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        // TODO: Implement projection pushdown
        Ok(None)
    }
}

/// Merge adjacent or nearby page byte ranges into single reads.
///
/// Pages whose gap is smaller than `gap_threshold` bytes are combined into one range,
/// trading a small amount of extra I/O for fewer round-trips (important for cloud stores).
///
/// # Arguments
/// * `pages` - All page groups in the layout (positional)
/// * `included` - Indices of pages to include (must be sorted ascending)
/// * `file_size` - Total file size (used to compute the last page's end offset)
/// * `gap_threshold` - Maximum gap in bytes before splitting into a new range (default: 256KB)
pub fn coalesce_page_ranges(
    pages: &[crate::page_map::PageGroup],
    included: &[usize],
    file_size: u64,
    gap_threshold: u64,
) -> Vec<(u64, u64)> {
    if included.is_empty() {
        return Vec::new();
    }

    let page_end = |i: usize| -> u64 {
        pages.get(i + 1).map(|p| p.physical_start).unwrap_or(file_size)
    };

    let mut ranges = Vec::new();
    let mut range_start = pages[included[0]].physical_start;
    let mut range_end = page_end(included[0]);

    for &i in &included[1..] {
        let next_start = pages[i].physical_start;
        let next_end = page_end(i);
        if next_start <= range_end + gap_threshold {
            // Merge: extend the current range to cover the gap and the next page
            range_end = next_end;
        } else {
            ranges.push((range_start, range_end));
            range_start = next_start;
            range_end = next_end;
        }
    }
    ranges.push((range_start, range_end));
    ranges
}

/// Parse raw line-oriented bytes into a RecordBatch.
///
/// For CSV: all lines are data rows (no header); the schema is used directly.
/// For JSONL: each non-empty line is a JSON object.
/// Applies column projection if provided.
fn parse_bytes_to_batch(
    bytes: &bytes::Bytes,
    schema: &SchemaRef,
    projection: &Option<Vec<usize>>,
    format: LineOrientedFormat,
) -> datafusion::common::Result<RecordBatch> {
    let cursor = Cursor::new(bytes.as_ref());
    let batch = match format {
        LineOrientedFormat::Csv => {
            // CSV page bytes contain data rows only (no header — pages are split after the header).
            // Build a synthetic header and parse.
            let header: String = {
                let mut h = String::new();
                let mut first = true;
                for field in schema.fields() {
                    if !first { h.push(','); }
                    h.push_str(field.name());
                    first = false;
                }
                h.push('\n');
                h
            };
            let mut csv_data = header;
            csv_data.push_str(&String::from_utf8_lossy(bytes.as_ref()));
            let cursor = Cursor::new(csv_data.as_bytes());
            let mut reader = CsvReaderBuilder::new(schema.clone())
                .with_header(true)
                .build(cursor)
                ?;
            // Collect all batches from this page range
            let mut batches = Vec::new();
            for result in reader {
                let b = result?;
                if b.num_rows() > 0 {
                    batches.push(b);
                }
            }
            if batches.is_empty() {
                return Ok(RecordBatch::new_empty(
                    project_schema(schema, projection.as_ref())
                        ?,
                ));
            }
            arrow::compute::concat_batches(&schema, &batches)
                ?
        }
        LineOrientedFormat::JsonLines => {
            let mut reader = JsonReaderBuilder::new(schema.clone())
                .build(cursor)
                ?;
            let mut batches = Vec::new();
            for result in &mut reader {
                let b = result?;
                if b.num_rows() > 0 {
                    batches.push(b);
                }
            }
            if batches.is_empty() {
                return Ok(RecordBatch::new_empty(
                    project_schema(schema, projection.as_ref())
                        ?,
                ));
            }
            arrow::compute::concat_batches(&schema, &batches)
                ?
        }
    };

    // Apply projection
    if let Some(proj) = projection {
        let projected_columns: Vec<_> = proj.iter().map(|&i| batch.column(i).clone()).collect();
        let projected_schema = Arc::new(
            schema.project(proj)?,
        );
        RecordBatch::try_new(projected_schema, projected_columns)
            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))
    } else {
        Ok(batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::test_config;

    use url::Url;

    #[test]
    fn test_byte_offset_sorting() {
        let byte_offsets = vec![1000u64, 100, 500];

        let file = ObjectStoreFile::from_url(
            &Url::parse("memory:///test.csv").expect("valid url"),
            test_config(),
        )
        .expect("valid file");
        let schema = Arc::new(arrow::datatypes::Schema::empty());
        let source = PageMapDataSource::new(&file, schema, byte_offsets, None, LineOrientedFormat::Csv);

        // Verify byte offsets are sorted
        assert_eq!(source.byte_offsets[0], 100);
        assert_eq!(source.byte_offsets[1], 500);
        assert_eq!(source.byte_offsets[2], 1000);
    }

    #[test]
    fn test_partition_statistics() {
        let byte_offsets = vec![100u64, 200];

        let file = ObjectStoreFile::from_url(
            &Url::parse("file:///test.csv").expect("valid url"),
            test_config(),
        )
        .expect("valid file");
        let schema = Arc::new(arrow::datatypes::Schema::empty());
        let source = PageMapDataSource::new(&file, schema, byte_offsets, None, LineOrientedFormat::Csv);

        let stats = source.partition_statistics(None).expect("stats");
        assert_eq!(stats.num_rows.get_value(), Some(&2));
    }

    #[test]
    fn test_batch_offsets_single_batch() {
        // Byte offsets that are close together should be batched (within 4KB read-ahead)
        let offsets = vec![1000u64, 2000, 3000];

        let batches = PageMapDataSource::batch_offsets(&offsets);

        // All three should be in one batch since they're within overlapping 4KB ranges
        assert_eq!(1, batches.len());
        let (start, end, batch_offsets) = &batches[0];
        assert_eq!(1000, *start);
        assert!(end >= &3000);
        assert_eq!(3, batch_offsets.len());
    }

    #[test]
    fn test_batch_offsets_multiple_batches() {
        // Byte offsets that are far apart should be in separate batches
        let offsets = vec![1000u64, 50000, 100000];

        let batches = PageMapDataSource::batch_offsets(&offsets);

        // Should be in separate batches since they're beyond 4KB read-ahead of each other
        assert_eq!(3, batches.len());
        assert_eq!(1, batches[0].2.len());
        assert_eq!(1, batches[1].2.len());
        assert_eq!(1, batches[2].2.len());
    }

    #[test]
    fn test_batch_offsets_mixed() {
        // Mix of close and far byte offsets
        let offsets = vec![1000u64, 2000, 50000, 51000, 100000];

        let batches = PageMapDataSource::batch_offsets(&offsets);

        // Should be in 3 batches
        assert_eq!(3, batches.len());
        assert_eq!(2, batches[0].2.len()); // First two offsets
        assert_eq!(2, batches[1].2.len()); // Next two offsets
        assert_eq!(1, batches[2].2.len()); // Last offset
    }

    #[test]
    fn test_extract_lines_csv() {
        // "value1,value2,value3\n" = 21 bytes, so line 2 starts at offset 21
        let csv_data = "value1,value2,value3\nvalue4,value5,value6\nvalue7,value8,value9\n";
        let bytes = csv_data.as_bytes();

        let lines = PageMapDataSource::extract_lines(bytes, 0, &[0, 21]);

        assert_eq!(2, lines.len());
        assert_eq!("value1,value2,value3", lines[0]);
        assert_eq!("value4,value5,value6", lines[1]);
    }

    #[test]
    fn test_extract_lines_single() {
        let csv_data = "single,line,data\n";
        let bytes = csv_data.as_bytes();

        let lines = PageMapDataSource::extract_lines(bytes, 0, &[0]);

        assert_eq!(1, lines.len());
        assert_eq!("single,line,data", lines[0]);
    }

    #[test]
    fn test_extract_lines_json() {
        // {"id":1,"name":"Alice"} = 23 bytes + \n = line 2 starts at offset 24
        let json_data = r#"{"id":1,"name":"Alice"}
{"id":2,"name":"Bob"}
{"id":3,"name":"Charlie"}
"#;
        let bytes = json_data.as_bytes();

        let lines = PageMapDataSource::extract_lines(bytes, 0, &[0, 24]);

        assert_eq!(2, lines.len());
        assert_eq!(r#"{"id":1,"name":"Alice"}"#, lines[0]);
        assert_eq!(r#"{"id":2,"name":"Bob"}"#, lines[1]);
    }

    #[test]
    fn test_extract_lines_no_trailing_newline() {
        // "line1\n" = 6 bytes, "line2\n" = 6 bytes, "partial" starts at 12
        let csv_data = "line1\nline2\npartial";
        let bytes = csv_data.as_bytes();

        let lines = PageMapDataSource::extract_lines(bytes, 0, &[0, 6, 12]);

        // Should only get 2 complete lines, not the partial one (no trailing newline)
        assert_eq!(2, lines.len());
        assert_eq!("line1", lines[0]);
        assert_eq!("line2", lines[1]);
    }

    #[test]
    fn test_extract_lines_ends_with_newline() {
        // "line1\n" = 6 bytes each
        let csv_data = "line1\nline2\nline3\n";
        let bytes = csv_data.as_bytes();

        let lines = PageMapDataSource::extract_lines(bytes, 0, &[0, 6, 12]);

        // Should get all 3 complete lines
        assert_eq!(3, lines.len());
        assert_eq!("line1", lines[0]);
        assert_eq!("line2", lines[1]);
        assert_eq!("line3", lines[2]);
    }

    #[test]
    fn test_extract_lines_empty_lines() {
        // "line1\n" = 6 bytes, "\n" = 1 byte at offset 6, "line3\n" starts at offset 7
        let csv_data = "line1\n\nline3\n";
        let bytes = csv_data.as_bytes();

        let lines = PageMapDataSource::extract_lines(bytes, 0, &[0, 6, 7]);

        // Empty line at offset 6 is trimmed and not included
        assert_eq!(2, lines.len());
        assert_eq!("line1", lines[0]);
        assert_eq!("line3", lines[1]);
    }

    #[test]
    fn test_extract_lines_non_adjacent_rows() {
        // Verify correct lines are extracted when rows aren't consecutive
        // Simulates an index lookup returning rows 1 and 3 but not row 2
        let data = "row1,data1\nrow2,data2\nrow3,data3\nrow4,data4\n";
        let bytes = data.as_bytes();
        // "row1,data1\n" = 11 bytes, "row2,data2\n" = 11 bytes, "row3,data3\n" starts at 22

        let lines = PageMapDataSource::extract_lines(bytes, 0, &[0, 22]);

        assert_eq!(2, lines.len());
        assert_eq!("row1,data1", lines[0]);
        assert_eq!("row3,data3", lines[1]);
    }

    #[test]
    fn test_extract_lines_with_batch_start_offset() {
        // Simulate reading a byte range starting at offset 100 in the file
        // The buffer contains data from file byte 100 onwards
        let data = "middle_row,value\nnext_row,value\n";
        let bytes = data.as_bytes();

        // Row offsets are absolute file positions
        let lines = PageMapDataSource::extract_lines(bytes, 100, &[100, 117]);

        assert_eq!(2, lines.len());
        assert_eq!("middle_row,value", lines[0]);
        assert_eq!("next_row,value", lines[1]);
    }

    #[test]
    fn test_extract_lines_truncated_line() {
        // Simulate read-ahead that doesn't capture the full line
        // Buffer has 20 bytes but the line is longer
        let data = "short\nthis_is_a_very";  // second line has no \n
        let bytes = data.as_bytes();

        let lines = PageMapDataSource::extract_lines(bytes, 0, &[0, 6]);

        // First line is complete, second line is truncated (no \n) so it's skipped
        assert_eq!(1, lines.len());
        assert_eq!("short", lines[0]);
    }

    #[test]
    fn test_extract_lines_offset_beyond_buffer() {
        // Row offset points beyond the fetched buffer
        let data = "only_line\n";
        let bytes = data.as_bytes();

        let lines = PageMapDataSource::extract_lines(bytes, 0, &[0, 500]);

        // First line extracted, second offset is beyond buffer and skipped
        assert_eq!(1, lines.len());
        assert_eq!("only_line", lines[0]);
    }

    #[test]
    fn test_batch_offsets_small_read_ahead() {
        // Verify that closely-spaced byte offsets batch together with 4KB read-ahead
        let offsets = vec![100u64, 200, 4000]; // 4000 still within 4KB of first (100+4096=4196)

        let batches = PageMapDataSource::batch_offsets(&offsets);

        // All three should batch together since they're within 4KB range
        assert_eq!(1, batches.len());
        assert_eq!(3, batches[0].2.len());
    }

    #[test]
    fn test_batch_offsets_just_beyond_read_ahead() {
        // Verify that byte offsets just beyond 4KB read-ahead create separate batches
        let offsets = vec![100u64, 5000]; // 5000 beyond 100+4096=4196 → new batch

        let batches = PageMapDataSource::batch_offsets(&offsets);

        assert_eq!(2, batches.len());
        assert_eq!(1, batches[0].2.len());
        assert_eq!(1, batches[1].2.len());
    }
}
