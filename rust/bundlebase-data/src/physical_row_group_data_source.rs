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

/// Custom DataSource that reads only specified rows from line-oriented files
/// by their byte offsets. Used for index-based query optimization to avoid full table scans.
/// Supports both CSV and JSON Lines formats.
pub struct PhysicalRowGroupDataSource {
    /// The source file
    file: ObjectStoreFile,
    /// Schema of the data (original full schema)
    schema: SchemaRef,
    /// Schema after projection is applied (computed at construction time)
    projected_schema: SchemaRef,
    /// Byte offsets of rows to read (sorted for sequential reading)
    byte_offsets: Vec<u64>,
    /// Number of rows (for statistics)
    num_rows: usize,
    /// Optional column projection (indices of columns to read)
    projection: Option<Vec<usize>>,
    /// Object store for reading file data
    object_store: Arc<dyn ObjectStore>,
    /// File format (CSV or JSON Lines)
    format: LineOrientedFormat,
}

impl PhysicalRowGroupDataSource {
    /// Create a PhysicalRowGroupDataSource from byte offsets.
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

impl Debug for PhysicalRowGroupDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhysicalRowGroupDataSource")
            .field("file", &self.file)
            .field("schema", &self.schema)
            .field("num_offsets", &self.byte_offsets.len())
            .field("projection", &self.projection)
            .field("format", &self.format)
            .finish()
    }
}

impl Display for PhysicalRowGroupDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PhysicalRowGroupDataSource[file={}, rows={}, format={:?}]",
            self.file.url(),
            self.num_rows,
            self.format
        )
    }
}

impl DataSource for PhysicalRowGroupDataSource {
    fn open(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        // Read rows by their byte offsets (works for CSV and JSON Lines)
        let schema = self.schema.clone();

        // Use pre-computed projected schema
        // This was computed in the constructor using project_schema()
        let output_schema = self.projected_schema.clone();

        log::debug!(
            "PhysicalRowGroupDataSource output schema has {} columns: {:?}",
            output_schema.fields().len(),
            output_schema
                .fields()
                .iter()
                .take(5)
                .map(|f| format!("{}:{}", f.name(), f.data_type()))
                .collect::<Vec<_>>()
        );

        let byte_offsets = self.byte_offsets.clone();
        let object_store = self.object_store.clone();
        let file_path = self.file.store_path().clone();
        let projection = self.projection.clone();
        let format = self.format;

        // Batch byte offsets for efficient fetching
        let batches = Self::batch_offsets(&byte_offsets);

        log::debug!(
            "Batched {} byte offsets into {} fetch operations for streaming",
            byte_offsets.len(),
            batches.len()
        );

        // Create async stream that yields one RecordBatch per fetch batch
        // This provides better memory usage than accumulating all data
        let stream = stream::iter(batches).then(move |(batch_start, batch_end, batch_offsets)| {
            let object_store = object_store.clone();
            let file_path = file_path.clone();
            let schema = schema.clone();
            let projection = projection.clone();

            async move {
                // Fetch the entire batch range in one ObjectStore call
                let range = GetRange::Bounded(batch_start..batch_end);
                let options = GetOptions {
                    range: Some(range),
                    ..Default::default()
                };

                let bytes = match object_store.get_opts(&file_path, options).await {
                    Ok(get_result) => get_result
                        .bytes()
                        .await
                        .map_err(|e| DataFusionError::External(Box::new(e)))?,
                    Err(e) => return Err(DataFusionError::External(Box::new(e))),
                };

                // Extract lines from this batch
                let lines = Self::extract_lines(&bytes, batch_start, &batch_offsets);

                // Build RecordBatch from lines based on format
                if lines.is_empty() {
                    // Return empty batch with correct schema (projected if projection exists)
                    let empty_schema = if let Some(proj) = &projection {
                        Arc::new(
                            schema
                                .project(proj)
                                .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?,
                        )
                    } else {
                        schema.clone()
                    };
                    return Ok(RecordBatch::new_empty(empty_schema));
                }

                let batch = match format {
                    LineOrientedFormat::Csv => {
                        // Estimate capacity: header + newlines + all lines
                        let lines_len: usize = lines.iter().map(|l| l.len() + 1).sum();
                        let header_len: usize = schema.fields().iter().map(|f| f.name().len() + 1).sum();
                        let mut csv_data = String::with_capacity(header_len + lines_len);

                        // Build header inline without intermediate Vec allocation
                        let mut first = true;
                        for field in schema.fields() {
                            if !first {
                                csv_data.push(',');
                            }
                            csv_data.push_str(field.name());
                            first = false;
                        }
                        csv_data.push('\n');

                        for line in lines {
                            csv_data.push_str(&line);
                            csv_data.push('\n');
                        }

                        // Parse CSV data into RecordBatch
                        let cursor = Cursor::new(csv_data.as_bytes());
                        let mut reader = CsvReaderBuilder::new(schema.clone())
                            .with_header(true)
                            .build(cursor)
                            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;

                        reader
                            .next()
                            .ok_or_else(|| {
                                DataFusionError::Internal("No batch produced".to_string())
                            })?
                            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?
                    }
                    LineOrientedFormat::JsonLines => {
                        // Pre-allocate capacity for all lines plus newlines
                        let total_len: usize = lines.iter().map(|l| l.len() + 1).sum();
                        let mut json_data = String::with_capacity(total_len);

                        for line in lines {
                            json_data.push_str(&line);
                            json_data.push('\n');
                        }

                        // Parse JSON Lines data into RecordBatch
                        let cursor = Cursor::new(json_data.as_bytes());
                        let mut reader = JsonReaderBuilder::new(schema.clone())
                            .build(cursor)
                            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;

                        reader
                            .next()
                            .ok_or_else(|| {
                                DataFusionError::Internal("No batch produced".to_string())
                            })?
                            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?
                    }
                };

                // Apply projection if specified
                let final_batch = if let Some(proj) = &projection {
                    log::debug!(
                        "Applying projection {:?} to batch with {} columns",
                        proj,
                        batch.num_columns()
                    );
                    let projected_columns: Vec<_> =
                        proj.iter().map(|&i| batch.column(i).clone()).collect();
                    let projected_schema = Arc::new(
                        schema
                            .project(proj)
                            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?,
                    );
                    let result = RecordBatch::try_new(projected_schema, projected_columns)
                        .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;
                    log::debug!(
                        "Created projected batch with {} columns",
                        result.num_columns()
                    );
                    result
                } else {
                    batch
                };

                Ok(final_batch)
            }
        });

        // Use output_schema which matches the actual schema of batches produced by the stream
        Ok(Box::pin(RecordBatchStreamAdapter::new(
            output_schema,
            stream,
        )))
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
        let source = PhysicalRowGroupDataSource::new(&file, schema, byte_offsets, None, LineOrientedFormat::Csv);

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
        let source = PhysicalRowGroupDataSource::new(&file, schema, byte_offsets, None, LineOrientedFormat::Csv);

        let stats = source.partition_statistics(None).expect("stats");
        assert_eq!(stats.num_rows.get_value(), Some(&2));
    }

    #[test]
    fn test_batch_offsets_single_batch() {
        // Byte offsets that are close together should be batched (within 4KB read-ahead)
        let offsets = vec![1000u64, 2000, 3000];

        let batches = PhysicalRowGroupDataSource::batch_offsets(&offsets);

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

        let batches = PhysicalRowGroupDataSource::batch_offsets(&offsets);

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

        let batches = PhysicalRowGroupDataSource::batch_offsets(&offsets);

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

        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 0, &[0, 21]);

        assert_eq!(2, lines.len());
        assert_eq!("value1,value2,value3", lines[0]);
        assert_eq!("value4,value5,value6", lines[1]);
    }

    #[test]
    fn test_extract_lines_single() {
        let csv_data = "single,line,data\n";
        let bytes = csv_data.as_bytes();

        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 0, &[0]);

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

        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 0, &[0, 24]);

        assert_eq!(2, lines.len());
        assert_eq!(r#"{"id":1,"name":"Alice"}"#, lines[0]);
        assert_eq!(r#"{"id":2,"name":"Bob"}"#, lines[1]);
    }

    #[test]
    fn test_extract_lines_no_trailing_newline() {
        // "line1\n" = 6 bytes, "line2\n" = 6 bytes, "partial" starts at 12
        let csv_data = "line1\nline2\npartial";
        let bytes = csv_data.as_bytes();

        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 0, &[0, 6, 12]);

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

        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 0, &[0, 6, 12]);

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

        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 0, &[0, 6, 7]);

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

        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 0, &[0, 22]);

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
        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 100, &[100, 117]);

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

        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 0, &[0, 6]);

        // First line is complete, second line is truncated (no \n) so it's skipped
        assert_eq!(1, lines.len());
        assert_eq!("short", lines[0]);
    }

    #[test]
    fn test_extract_lines_offset_beyond_buffer() {
        // Row offset points beyond the fetched buffer
        let data = "only_line\n";
        let bytes = data.as_bytes();

        let lines = PhysicalRowGroupDataSource::extract_lines(bytes, 0, &[0, 500]);

        // First line extracted, second offset is beyond buffer and skipped
        assert_eq!(1, lines.len());
        assert_eq!("only_line", lines[0]);
    }

    #[test]
    fn test_batch_offsets_small_read_ahead() {
        // Verify that closely-spaced byte offsets batch together with 4KB read-ahead
        let offsets = vec![100u64, 200, 4000]; // 4000 still within 4KB of first (100+4096=4196)

        let batches = PhysicalRowGroupDataSource::batch_offsets(&offsets);

        // All three should batch together since they're within 4KB range
        assert_eq!(1, batches.len());
        assert_eq!(3, batches[0].2.len());
    }

    #[test]
    fn test_batch_offsets_just_beyond_read_ahead() {
        // Verify that byte offsets just beyond 4KB read-ahead create separate batches
        let offsets = vec![100u64, 5000]; // 5000 beyond 100+4096=4196 → new batch

        let batches = PhysicalRowGroupDataSource::batch_offsets(&offsets);

        assert_eq!(2, batches.len());
        assert_eq!(1, batches[0].2.len());
        assert_eq!(1, batches[1].2.len());
    }
}
