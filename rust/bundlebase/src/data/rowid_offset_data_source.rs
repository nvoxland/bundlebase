use crate::data::RowId;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadFile;
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

/// Custom DataSource that reads only specified rows by their RowIds from line-oriented files
/// Used for index-based query optimization to avoid full table scans
/// Supports both CSV and JSON Lines formats
pub struct RowIdOffsetDataSource {
    /// The source file
    file: ObjectStoreFile,
    /// Schema of the data (original full schema)
    schema: SchemaRef,
    /// Schema after projection is applied (computed at construction time)
    projected_schema: SchemaRef,
    /// List of RowIds to read (sorted by offset for sequential reading)
    row_ids: Vec<RowId>,
    /// Optional column projection (indices of columns to read)
    projection: Option<Vec<usize>>,
    /// Object store for reading file data
    object_store: Arc<dyn ObjectStore>,
    /// File format (CSV or JSON Lines)
    format: LineOrientedFormat,
}

impl RowIdOffsetDataSource {
    /// Create a new RowIdOffsetDataSource
    ///
    /// # Arguments
    /// * `file` - The source file
    /// * `schema` - Schema of the data
    /// * `row_ids` - List of RowIds to read
    /// * `projection` - Optional column projection
    /// * `format` - File format (CSV or JSON Lines)
    pub fn new(
        file: &ObjectStoreFile,
        schema: SchemaRef,
        row_ids: Vec<RowId>,
        projection: Option<Vec<usize>>,
        format: LineOrientedFormat,
    ) -> Self {
        // Sort row_ids by offset for sequential reading
        let mut sorted_ids = row_ids;
        sorted_ids.sort_by_key(|id| id.offset());

        // Get object store from URL
        let object_store = file.store();

        // Compute projected schema using DataFusion's utility
        // This ensures eq_properties() returns the correct schema that matches what open() produces
        let projected_schema =
            project_schema(&schema, projection.as_ref()).expect("Failed to project schema");

        Self {
            file: file.clone(),
            schema,
            projected_schema,
            row_ids: sorted_ids,
            projection,
            object_store,
            format,
        }
    }

    /// Extract all complete lines from a byte range
    /// Works for both CSV and JSON Lines since both are line-oriented
    /// Returns only complete lines (those ending with newline)
    /// Partial lines after the last newline are ignored
    fn extract_lines(bytes: &[u8], row_offsets: &[u64]) -> Vec<String> {
        if bytes.is_empty() || row_offsets.is_empty() {
            return Vec::new();
        }

        let text = String::from_utf8_lossy(bytes);
        let mut lines = Vec::new();
        let mut current_pos = 0;

        // For each expected row offset, extract the next complete line
        for _ in row_offsets {
            if current_pos >= text.len() {
                break; // No more data
            }

            // Extract the line from current position to next newline
            if let Some(relative_end) = text[current_pos..].find('\n') {
                let end = current_pos + relative_end;
                let trimmed = text[current_pos..end].trim();
                if !trimmed.is_empty() {
                    lines.push(trimmed.to_string());
                }
                current_pos = end + 1;
            } else {
                // No newline found - this is a partial line, don't include it
                break;
            }
        }

        lines
    }

    /// Group row IDs into batches for efficient fetching
    /// Rows are batched together if they fall within the same or overlapping byte ranges
    fn batch_row_ids(row_ids: &[RowId]) -> Vec<(u64, u64, Vec<u64>)> {
        if row_ids.is_empty() {
            return Vec::new();
        }

        let mut batches = Vec::new();
        let mut current_start = row_ids[0].offset();
        let mut current_end = current_start + (row_ids[0].size_mb() as u64 * 1024 * 1024);
        let mut current_offsets = vec![row_ids[0].offset()];

        for row_id in &row_ids[1..] {
            let row_start = row_id.offset();
            let row_end = row_start + (row_id.size_mb() as u64 * 1024 * 1024);

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

impl Debug for RowIdOffsetDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowIdOffsetDataSource")
            .field("file", &self.file)
            .field("schema", &self.schema)
            .field("num_row_ids", &self.row_ids.len())
            .field("projection", &self.projection)
            .field("format", &self.format)
            .finish()
    }
}

impl Display for RowIdOffsetDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RowIdOffsetDataSource[file={}, rows={}, format={:?}]",
            self.file.url(),
            self.row_ids.len(),
            self.format
        )
    }
}

impl DataSource for RowIdOffsetDataSource {
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
            "RowIdOffsetDataSource output schema has {} columns: {:?}",
            output_schema.fields().len(),
            output_schema
                .fields()
                .iter()
                .take(5)
                .map(|f| format!("{}:{}", f.name(), f.data_type()))
                .collect::<Vec<_>>()
        );

        let row_ids = self.row_ids.clone();
        let object_store = self.object_store.clone();
        let file_path = self.file.store_path().clone();
        let projection = self.projection.clone();
        let format = self.format;

        // Batch row IDs for efficient fetching
        let batches = Self::batch_row_ids(&row_ids);

        log::debug!(
            "Batched {} row IDs into {} fetch operations for streaming",
            row_ids.len(),
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
                let lines = Self::extract_lines(&bytes, &batch_offsets);

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
        stats.num_rows = datafusion::common::stats::Precision::Exact(self.row_ids.len());
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
    use crate::data::ObjectId;
    use crate::BundleConfig;
    use url::Url;

    #[test]
    fn test_row_id_sorting() {
        // Create RowIds with different offsets
        let block_id = ObjectId::from(42);
        let row_ids = vec![
            RowId::new(&block_id, 1000, 10),
            RowId::new(&block_id, 100, 10),
            RowId::new(&block_id, 500, 10),
        ];

        let file = ObjectStoreFile::from_url(
            &Url::parse("memory:///test.csv").unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        let schema = Arc::new(arrow::datatypes::Schema::empty());
        let source = RowIdOffsetDataSource::new(&file, schema, row_ids, None, LineOrientedFormat::Csv);

        // Verify row_ids are sorted by offset
        assert_eq!(source.row_ids[0].offset(), 100);
        assert_eq!(source.row_ids[1].offset(), 500);
        assert_eq!(source.row_ids[2].offset(), 1000);
    }

    #[test]
    fn test_partition_statistics() {
        let block_id = ObjectId::from(42);
        let row_ids = vec![
            RowId::new(&block_id, 100, 10),
            RowId::new(&block_id, 200, 10),
        ];

        let file = ObjectStoreFile::from_url(
            &Url::parse("file:///test.csv").unwrap(),
            BundleConfig::default().into(),
        )
        .unwrap();
        let schema = Arc::new(arrow::datatypes::Schema::empty());
        let source = RowIdOffsetDataSource::new(&file, schema, row_ids, None, LineOrientedFormat::Csv);

        let stats = source.partition_statistics(None).unwrap();
        assert_eq!(stats.num_rows.get_value(), Some(&2));
    }

    #[test]
    fn test_batch_row_ids_single_batch() {
        // RowIds that are close together should be batched
        let block_id = ObjectId::from(42);
        let row_ids = vec![
            RowId::new(&block_id, 1000, 1), // offset 1000, size ~1MB
            RowId::new(&block_id, 2000, 1), // offset 2000, within 1MB range of first
            RowId::new(&block_id, 3000, 1), // offset 3000, within range
        ];

        let batches = RowIdOffsetDataSource::batch_row_ids(&row_ids);

        // All three should be in one batch since they're within overlapping ranges
        assert_eq!(1, batches.len());
        let (start, end, offsets) = &batches[0];
        assert_eq!(1000, *start);
        assert!(end >= &3000);
        assert_eq!(3, offsets.len());
    }

    #[test]
    fn test_batch_row_ids_multiple_batches() {
        // RowIds that are far apart should be in separate batches
        let block_id = ObjectId::from(42);
        let row_ids = vec![
            RowId::new(&block_id, 1000, 1),     // offset 1000, size ~1MB
            RowId::new(&block_id, 5000000, 1),  // offset 5MB, far from first
            RowId::new(&block_id, 10000000, 1), // offset 10MB, far from second
        ];

        let batches = RowIdOffsetDataSource::batch_row_ids(&row_ids);

        // Should be in separate batches since they're far apart
        assert_eq!(3, batches.len());
        assert_eq!(1, batches[0].2.len());
        assert_eq!(1, batches[1].2.len());
        assert_eq!(1, batches[2].2.len());
    }

    #[test]
    fn test_batch_row_ids_mixed() {
        // Mix of close and far RowIds
        let block_id = ObjectId::from(42);
        let row_ids = vec![
            RowId::new(&block_id, 1000, 1),     // Batch 1
            RowId::new(&block_id, 2000, 1),     // Batch 1
            RowId::new(&block_id, 5000000, 1),  // Batch 2
            RowId::new(&block_id, 5001000, 1),  // Batch 2
            RowId::new(&block_id, 10000000, 1), // Batch 3
        ];

        let batches = RowIdOffsetDataSource::batch_row_ids(&row_ids);

        // Should be in 3 batches
        assert_eq!(3, batches.len());
        assert_eq!(2, batches[0].2.len()); // First two rows
        assert_eq!(2, batches[1].2.len()); // Next two rows
        assert_eq!(1, batches[2].2.len()); // Last row
    }

    #[test]
    fn test_extract_lines_csv() {
        let csv_data = "value1,value2,value3\nvalue4,value5,value6\nvalue7,value8,value9\n";
        let bytes = csv_data.as_bytes();

        // Extract 2 lines
        let lines = RowIdOffsetDataSource::extract_lines(bytes, &[0, 0]);

        assert_eq!(2, lines.len());
        assert_eq!("value1,value2,value3", lines[0]);
        assert_eq!("value4,value5,value6", lines[1]);
    }

    #[test]
    fn test_extract_lines_single() {
        let csv_data = "single,line,data\n";
        let bytes = csv_data.as_bytes();

        let lines = RowIdOffsetDataSource::extract_lines(bytes, &[0]);

        assert_eq!(1, lines.len());
        assert_eq!("single,line,data", lines[0]);
    }

    #[test]
    fn test_extract_lines_json() {
        let json_data = r#"{"id":1,"name":"Alice"}
{"id":2,"name":"Bob"}
{"id":3,"name":"Charlie"}
"#;
        let bytes = json_data.as_bytes();

        // Extract 2 JSON lines
        let lines = RowIdOffsetDataSource::extract_lines(bytes, &[0, 0]);

        assert_eq!(2, lines.len());
        assert_eq!(r#"{"id":1,"name":"Alice"}"#, lines[0]);
        assert_eq!(r#"{"id":2,"name":"Bob"}"#, lines[1]);
    }

    #[test]
    fn test_extract_lines_no_trailing_newline() {
        // Data without trailing newline - partial line should be ignored
        let csv_data = "line1\nline2\npartial";
        let bytes = csv_data.as_bytes();

        let lines = RowIdOffsetDataSource::extract_lines(bytes, &[0, 0, 0]);

        // Should only get 2 complete lines, not the partial one
        assert_eq!(2, lines.len());
        assert_eq!("line1", lines[0]);
        assert_eq!("line2", lines[1]);
    }

    #[test]
    fn test_extract_lines_ends_with_newline() {
        // Data ending with newline - should include the line before it
        let csv_data = "line1\nline2\nline3\n";
        let bytes = csv_data.as_bytes();

        let lines = RowIdOffsetDataSource::extract_lines(bytes, &[0, 0, 0]);

        // Should get all 3 complete lines
        assert_eq!(3, lines.len());
        assert_eq!("line1", lines[0]);
        assert_eq!("line2", lines[1]);
        assert_eq!("line3", lines[2]);
    }

    #[test]
    fn test_extract_lines_empty_lines() {
        // Data with empty lines (just newlines)
        let csv_data = "line1\n\nline3\n";
        let bytes = csv_data.as_bytes();

        let lines = RowIdOffsetDataSource::extract_lines(bytes, &[0, 0, 0]);

        // Empty lines are trimmed and not included
        assert_eq!(2, lines.len());
        assert_eq!("line1", lines[0]);
        assert_eq!("line3", lines[1]);
    }
}
