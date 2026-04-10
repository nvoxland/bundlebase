use crate::DataContext;
use crate::plugin::file_reader::{FileFormatConfig, FilePlugin, FileReader};
use crate::plugin::ReaderPlugin;
use crate::attach_format::AttachFormat;
use crate::{BlockId, DataReader, LineOrientedFormat, PhysicalRowGroupDataSource, RowId};
use crate::physical_row_group_layout::{PhysicalRowGroupLayout, resolve_row_numbers_to_byte_offsets};
use bundlebase_io::plugin::object_store::ObjectStoreFile;
use bundlebase_io::IOReadWriteDir;
use bundlebase_common::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use bytes::Buf;
use datafusion::common::config::CsvOptions;
use datafusion::common::stats::Precision;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::file_format::csv::CsvFormat;
use datafusion::datasource::file_format::FileFormat;
use datafusion::datasource::physical_plan::{CsvSource, FileSource};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use url::Url;

/// Configuration for CSV format.
///
/// Carries an optional `newlines_in_values` flag that is shared (via `Arc<AtomicBool>`)
/// across clones so that `CsvReader::read_schema()` can enable it on retry and
/// subsequent calls to `file_format()`/`file_source()` see the updated value.
#[derive(Debug, Clone)]
pub struct CsvFormatConfig {
    newlines_in_values: Arc<AtomicBool>,
    delimiter: u8,
    extensions: &'static [&'static str],
}

impl Default for CsvFormatConfig {
    fn default() -> Self {
        Self {
            newlines_in_values: Arc::new(AtomicBool::new(false)),
            delimiter: b',',
            extensions: &[".csv"],
        }
    }
}

impl CsvFormatConfig {
    /// Create a TSV config (tab-delimited).
    pub fn tsv() -> Self {
        Self {
            newlines_in_values: Arc::new(AtomicBool::new(false)),
            delimiter: b'\t',
            extensions: &[".tsv"],
        }
    }

    /// Create a config with specific options.
    pub fn from_read_options(opts: &HashMap<String, String>, delimiter: u8) -> Self {
        let niv = opts
            .get("newlines_in_values")
            .map(|v| v == "true")
            .unwrap_or(false);
        Self {
            newlines_in_values: Arc::new(AtomicBool::new(niv)),
            delimiter,
            extensions: if delimiter == b'\t' { &[".tsv"] } else { &[".csv"] },
        }
    }

    fn csv_options(&self) -> CsvOptions {
        let mut opts = CsvOptions::default().with_delimiter(self.delimiter);
        if self.newlines_in_values.load(Ordering::Acquire) {
            opts = opts.with_newlines_in_values(true);
        }
        opts
    }
}

impl FileFormatConfig for CsvFormatConfig {
    fn extensions(&self) -> &'static [&'static str] {
        self.extensions
    }

    fn file_format(&self) -> Arc<dyn FileFormat> {
        Arc::new(CsvFormat::default().with_options(self.csv_options()))
    }

    fn file_source(&self, schema: SchemaRef) -> Arc<dyn FileSource> {
        Arc::new(CsvSource::new(schema).with_csv_options(self.csv_options()))
    }

    fn line_oriented_format(&self) -> Option<LineOrientedFormat> {
        Some(LineOrientedFormat::Csv)
    }
}

/// CSV plugin - uses generic FilePlugin and creates CsvReader
pub struct CsvPlugin {
    config: CsvFormatConfig,
}

impl Default for CsvPlugin {
    fn default() -> Self {
        Self {
            config: CsvFormatConfig::default(),
        }
    }
}

#[async_trait]
impl ReaderPlugin for CsvPlugin {
    async fn reader(
        &self,
        source: &str,
        block_id: &BlockId,
        bundle: &dyn DataContext,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
        read_options: Option<&HashMap<String, String>>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError> {
        let lower = source.to_lowercase();
        if !lower.ends_with(".csv") {
            return Ok(None);
        }

        // Use stored read_options if provided, otherwise default config
        let config = match read_options {
            Some(opts) if !opts.is_empty() => CsvFormatConfig::from_read_options(opts, b','),
            _ => self.config.clone(),
        };
        let plugin = FilePlugin::new(config);

        let reader = plugin
            .reader(source, bundle, schema, expected_version)
            .await?;
        let layout = match layout {
            None => None,
            Some(x) => Some(ObjectStoreFile::from_str(
                x.as_str(),
                bundle.data_context_dir().as_ref(),
                bundle.config_provider(),
            )?),
        };
        Ok(Some(Arc::new(CsvReader::new(reader, block_id, &layout, AttachFormat::Csv))))
    }
}

pub struct CsvReader {
    inner: FileReader<CsvFormatConfig>,
    block_id: BlockId,
    layout: Option<ObjectStoreFile>,
    attach_format: AttachFormat,
}

impl CsvReader {
    pub fn new(
        inner: FileReader<CsvFormatConfig>,
        block_id: &BlockId,
        layout: &Option<ObjectStoreFile>,
        attach_format: AttachFormat,
    ) -> Self {
        Self {
            inner,
            block_id: *block_id,
            layout: layout.clone(),
            attach_format,
        }
    }
}

impl std::fmt::Debug for CsvReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CsvReader")
            .field("inner", &self.inner)
            .field("block_id", &self.block_id)
            .field("layout", &self.layout)
            .field("attach_format", &self.attach_format)
            .finish()
    }
}

/// Check if an error is a LineDelimiter "unterminated string" error from DataFusion's
/// CSV line splitting, which indicates the file needs `newlines_in_values` enabled.
///
/// This relies on string-matching the error message, which is fragile if
/// upstream error wording changes. The `test_is_line_delimiter_error_detection`
/// test verifies the expected format still matches.
///
/// Verified against: DataFusion 52 / object_store crate.
/// If upgrading DataFusion, run the `test_is_line_delimiter_error_detection` test
/// to confirm the error message format has not changed.
fn is_line_delimiter_error(err: &BundlebaseError) -> bool {
    let msg = err.to_string();
    msg.contains("LineDelimiter") && msg.contains("unterminated string")
}

/// Trim leading/trailing whitespace from all field names in a schema.
///
/// CSV files commonly have whitespace in column headers (e.g., `col1, col2, col3`
/// producing `"col1"`, `" col2"`, `" col3"`). Neither DataFusion's `CsvOptions`
/// nor Arrow's CSV reader provides a trim option for headers, so we post-process
/// the schema here. Short-circuits if no names need trimming.
fn trim_schema_field_names(schema: SchemaRef) -> SchemaRef {
    let needs_trimming = schema
        .fields()
        .iter()
        .any(|f| f.name() != f.name().trim());
    if !needs_trimming {
        return schema;
    }

    let trimmed_fields: Vec<_> = schema
        .fields()
        .iter()
        .map(|f| {
            let trimmed = f.name().trim();
            if trimmed != f.name() {
                Arc::new(f.as_ref().clone().with_name(trimmed))
            } else {
                Arc::clone(f)
            }
        })
        .collect();

    Arc::new(Schema::new_with_metadata(
        trimmed_fields,
        schema.metadata().clone(),
    ))
}

/// Read just the CSV header row and return an all-Utf8 schema.
///
/// CSV is a text format, so all columns are naturally text. We skip type
/// inference entirely because it samples only the first rows and can be
/// wrong for later data.
async fn read_csv_header(
    store: &Arc<dyn object_store::ObjectStore>,
    path: &object_store::path::Path,
    delimiter: u8,
) -> Result<SchemaRef, BundlebaseError> {
    use object_store::GetOptions;

    // Read first 64KB — more than enough for the header row
    let opts = GetOptions {
        range: Some((0..65_536).into()),
        ..Default::default()
    };
    let result = store.get_opts(path, opts).await?;
    let bytes = result.bytes().await?;

    // Parse just the header (0 data rows) to get column names
    let format = arrow::csv::reader::Format::default()
        .with_header(true)
        .with_delimiter(delimiter);
    let (schema, _) = format.infer_schema(bytes.reader(), Some(0))?;

    // Build an all-Utf8 schema from the column names
    let fields: Vec<_> = schema
        .fields()
        .iter()
        .map(|f| Arc::new(Field::new(f.name(), DataType::Utf8, true)))
        .collect();
    Ok(Arc::new(Schema::new(fields)))
}

#[async_trait]
impl DataReader for CsvReader {
    fn url(&self) -> &Url {
        self.inner.url()
    }

    fn block_id(&self) -> BlockId {
        self.block_id
    }

    fn format(&self) -> crate::attach_format::AttachFormat {
        self.attach_format.clone()
    }

    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError> {
        // Read only the header row — all CSV columns are text.
        // We intentionally skip DataFusion's type inference because it samples
        // only the first rows, and later rows may contain values that don't
        // match the inferred types.
        let store = self.inner.object_store();
        let path = object_store::path::Path::parse(self.inner.url().path())?;
        let delimiter = self.inner.config().delimiter;
        match read_csv_header(&store, &path, delimiter).await {
            Ok(schema) => Ok(Some(trim_schema_field_names(schema))),
            Err(e) if is_line_delimiter_error(&e) => {
                log::info!(
                    "CSV file {} triggered LineDelimiter error; reading header from sample",
                    self.inner.url()
                );
                // Enable newlines_in_values for subsequent data reads
                self.inner
                    .config()
                    .newlines_in_values
                    .store(true, Ordering::Release);

                let schema = read_csv_header(&store, &path, delimiter).await?;
                Ok(Some(trim_schema_field_names(schema)))
            }
            Err(e) => Err(e),
        }
    }

    fn read_options(&self) -> HashMap<String, String> {
        let mut opts = HashMap::new();
        if self
            .inner
            .config()
            .newlines_in_values
            .load(Ordering::Acquire)
        {
            opts.insert("newlines_in_values".to_string(), "true".to_string());
        }
        opts
    }

    async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
        row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError> {
        if let Some(ids) = row_ids {
            let row_numbers: Vec<u32> = ids.iter().map(|id| id.row_number()).collect();
            let byte_offsets = resolve_row_numbers_to_byte_offsets(
                self.inner.file().as_object_store_file(),
                self.layout.as_ref(),
                &row_numbers,
                true, // skip CSV header
            )
            .await
            .map_err(|e| DataFusionError::External(e))?;

            let schema = self
                .inner
                .read_schema()
                .await
                .map_err(|e| DataFusionError::External(e))?
                .ok_or_else(|| DataFusionError::Internal("No schema available".to_string()))?;

            return Ok(Arc::new(PhysicalRowGroupDataSource::new(
                self.inner.file().as_object_store_file(),
                schema,
                byte_offsets,
                projection.cloned(),
                LineOrientedFormat::Csv,
            )));
        }
        self.inner
            .data_source(projection, filters, limit)
            .await
    }

    async fn read_version(&self) -> Result<String, BundlebaseError> {
        self.inner.version().await
    }

    async fn read_statistics(&self) -> Result<Option<Statistics>, BundlebaseError> {
        use object_store::GetOptions;

        let object_store = self.inner.object_store();
        let path = object_store::path::Path::parse(self.inner.url().path())?;

        // Stream the file counting newlines — O(1) memory, no buffering.
        let get_result = object_store.get_opts(&path, GetOptions::default()).await?;
        let mut stream = get_result.into_stream();

        let mut row_count: usize = 0;
        let mut file_size: usize = 0;
        let mut last_byte: u8 = b'\n';
        let mut header_seen = false;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Box::new(e) as BundlebaseError)?;
            for &b in chunk.iter() {
                if b == b'\n' {
                    if header_seen {
                        row_count += 1;
                    } else {
                        header_seen = true;
                    }
                }
            }
            file_size += chunk.len();
            if !chunk.is_empty() {
                last_byte = chunk[chunk.len() - 1];
            }
        }

        // Account for final line without trailing newline
        if file_size > 0 && last_byte != b'\n' && header_seen {
            row_count += 1;
        }

        let stats = Statistics {
            num_rows: Precision::Exact(row_count),
            total_byte_size: Precision::Exact(file_size),
            ..Default::default()
        };

        Ok(Some(stats))
    }

    async fn column_stats(&self) -> Result<Vec<crate::physical_row_group_layout::ColumnStats>, BundlebaseError> {
        let layout_file = match &self.layout {
            Some(f) => f,
            None => return Ok(vec![]),
        };
        let layout = crate::physical_row_group_layout::PhysicalRowGroupLayout::load(layout_file).await?;
        Ok(layout.column_stats)
    }

    async fn build_layout(
        &self,
        data_dir: &dyn IOReadWriteDir,
    ) -> Result<Option<Box<dyn bundlebase_io::IOReadFile>>, BundlebaseError> {
        use crate::physical_row_group_layout::DEFAULT_PAGE_SIZE;
        use futures::stream;

        let object_store = self.inner.object_store();
        let path = object_store::path::Path::parse(self.inner.url().path())?;
        let delimiter = self.inner.config().delimiter;

        // Read file bytes once — used for both page layout and column stats.
        let get_result = object_store.get_opts(&path, object_store::GetOptions::default()).await?;
        let mut file_stream = get_result.into_stream();
        let mut buffer = Vec::new();
        while let Some(chunk) = file_stream.next().await {
            let chunk = chunk.map_err(|e| Box::new(e) as BundlebaseError)?;
            buffer.extend_from_slice(&chunk);
        }

        // Build page layout first (no stats yet) to get page boundaries.
        let initial_layout = match PhysicalRowGroupLayout::build(&buffer, true, DEFAULT_PAGE_SIZE, vec![]) {
            None => return Ok(None),
            Some(l) => l,
        };
        let page_row_starts: Vec<u32> = initial_layout.pages.iter().map(|p| p.row_begin).collect();

        // Compute column stats with page-level tracking.
        let column_stats = self.compute_column_stats_sync(&buffer, &page_row_starts, delimiter)?;

        // Assemble final layout with stats and write.
        let layout = PhysicalRowGroupLayout { column_stats, ..initial_layout };
        let index_bytes = layout.serialize()?;
        let data_stream = Box::pin(stream::once(async move { Ok::<_, std::io::Error>(index_bytes) }));
        let address = bundlebase_common::ContentAddress::with_sub_type(
            bundlebase_common::ContentCategory::Block,
            "layout",
            bundlebase_common::ContentFormat::Pagemap,
        )?;
        let result = data_dir.write_stream(data_stream, &address).await?;
        Ok(Some(result.file))
    }

    async fn data_source_filtered_pages(
        &self,
        projection: Option<&Vec<usize>>,
        filters: &[datafusion::logical_expr::Expr],
        _limit: Option<usize>,
    ) -> Result<Option<Arc<dyn datafusion::datasource::source::DataSource>>, BundlebaseError> {
        use bundlebase_io::IOReadFile;
        use crate::layout_cache::GLOBAL_LAYOUT_CACHE;
        use crate::page_filter::{extract_lower_bound, extract_upper_bound, is_value_above_bound, is_value_below_bound, prune_exact_with_bloom, prune_prefix, prune_range};
        use crate::physical_row_group_layout::PhysicalRowGroupLayout;
        use bundlebase_index::{FilterAnalyzer, IndexPredicate};

        let layout_file = match &self.layout {
            Some(f) => f,
            None => return Ok(None),
        };

        // Load layout from cache (page stats live in layout.column_stats[col].page_stats)
        let layout_url = layout_file.url().clone();
        let layout = if let Some(cached) = GLOBAL_LAYOUT_CACHE.get(&layout_url) {
            cached
        } else {
            let loaded = PhysicalRowGroupLayout::load(layout_file).await?;
            let arc = Arc::new(loaded);
            GLOBAL_LAYOUT_CACHE.insert(layout_url, arc.clone());
            arc
        };

        if layout.pages.len() <= 1 {
            // Single page — no pruning possible.
            return Ok(None);
        }

        let schema = match self.inner.read_schema().await? {
            Some(s) => s,
            None => return Ok(None),
        };

        let indexable = FilterAnalyzer::extract_indexable(filters);
        if indexable.is_empty() {
            return Ok(None);
        }

        // Determine which pages to include. Start with all included.
        let num_pages = layout.pages.len();
        let mut include = vec![true; num_pages];

        for filter in &indexable {
            // Find column index in schema by name
            let col_idx = match schema.fields().iter().position(|f| f.name() == &filter.column) {
                Some(i) => i,
                None => continue,
            };

            let col_stats = match layout.column_stats.get(col_idx) {
                Some(s) => s,
                None => continue,
            };

            if col_stats.page_stats.is_empty() {
                continue; // No per-page stats for this column
            }

            let is_increasing = col_stats.is_strictly_increasing;
            let is_decreasing = col_stats.is_strictly_decreasing;

            for (page_idx, page_stat) in col_stats.page_stats.iter().enumerate() {
                if !include[page_idx] {
                    continue; // Already excluded
                }

                let bloom = page_stat.bloom_filter.as_deref();
                let can_prune = match &filter.predicate {
                    IndexPredicate::Exact(val) => {
                        prune_exact_with_bloom(val, page_stat.min.as_ref(), page_stat.max.as_ref(), bloom)
                    }
                    IndexPredicate::Range { min: fmin, max: fmax } => {
                        prune_range(fmin, fmax, page_stat.min.as_ref(), page_stat.max.as_ref())
                    }
                    IndexPredicate::In(vals) => {
                        vals.iter().all(|v| prune_exact_with_bloom(v, page_stat.min.as_ref(), page_stat.max.as_ref(), bloom))
                    }
                    // Per-page null counts are not tracked; null pruning only works at block level
                    IndexPredicate::IsNull | IndexPredicate::IsNotNull => false,
                    IndexPredicate::Prefix(prefix) => {
                        prune_prefix(prefix, page_stat.min.as_ref(), page_stat.max.as_ref())
                    }
                };

                if can_prune {
                    include[page_idx] = false;
                }
            }

            // Monotonic early-stop: for strictly increasing columns with an upper bound,
            // once page_min >= upper_bound all subsequent pages are also out of range.
            if is_increasing {
                if let Some(upper) = extract_upper_bound(&filter.predicate) {
                    let mut past_range = false;
                    for (page_idx, page_stat) in col_stats.page_stats.iter().enumerate() {
                        if past_range {
                            include[page_idx] = false;
                        } else if let Some(ref pmin) = page_stat.min {
                            if is_value_above_bound(&upper, pmin) {
                                include[page_idx] = false;
                                past_range = true;
                            }
                        }
                    }
                }
            }
            // For strictly decreasing columns with a lower bound, once page_max <= lower_bound
            // all subsequent pages are also out of range.
            if is_decreasing {
                if let Some(lower) = extract_lower_bound(&filter.predicate) {
                    let mut past_range = false;
                    for (page_idx, page_stat) in col_stats.page_stats.iter().enumerate() {
                        if past_range {
                            include[page_idx] = false;
                        } else if let Some(ref pmax) = page_stat.max {
                            if is_value_below_bound(&lower, pmax) {
                                include[page_idx] = false;
                                past_range = true;
                            }
                        }
                    }
                }
            }
        }

        // If all pages are included, no pruning happened — fall through to normal path.
        if include.iter().all(|&b| b) {
            return Ok(None);
        }
        // If no pages are included, this is covered by block-level pruning (should be rare).
        // Return an empty page set — the caller handles the empty DataSource.

        // Collect included page indices.
        let included: Vec<usize> = include.iter().enumerate()
            .filter(|(_, &inc)| inc)
            .map(|(i, _)| i)
            .collect();

        if included.is_empty() {
            return Ok(None); // Block pruning should have caught this
        }

        // Coalesce adjacent / nearby pages into single range reads (256KB gap threshold).
        const GAP_THRESHOLD: u64 = 256 * 1024;
        let page_ranges = crate::physical_row_group_data_source::coalesce_page_ranges(
            &layout.pages,
            &included,
            layout.file_size,
            GAP_THRESHOLD,
        );

        log::debug!(
            "CSV page-filter: {} included pages → {} coalesced ranges",
            included.len(),
            page_ranges.len()
        );

        let full_schema = schema;
        Ok(Some(Arc::new(PhysicalRowGroupDataSource::from_page_ranges(
            self.inner.file().as_object_store_file(),
            full_schema,
            page_ranges,
            projection.cloned(),
            LineOrientedFormat::Csv,
        ))))
    }
}

impl CsvReader {
    /// Compute per-column statistics (synchronously) from in-memory CSV bytes.
    ///
    /// `page_row_starts` — `layout.pages[i].row_begin` for each page, used to populate
    /// per-page min/max in the builder.
    fn compute_column_stats_sync(
        &self,
        bytes: &[u8],
        page_row_starts: &[u32],
        delimiter: u8,
    ) -> Result<Vec<crate::physical_row_group_layout::ColumnStats>, BundlebaseError> {
        use crate::column_stats_builder::ColumnStatsBuilder;
        use arrow::datatypes::{DataType, Field, Schema};

        // Parse just the header to get column names for the all-Utf8 schema.
        let format = arrow::csv::reader::Format::default()
            .with_header(true)
            .with_delimiter(delimiter);
        let (raw_schema, _) = format.infer_schema(std::io::Cursor::new(bytes), Some(0))
            .map_err(|e| BundlebaseError::from(format!("CSV header parse error: {}", e)))?;
        let col_count = raw_schema.fields().len();
        if col_count == 0 {
            return Ok(Vec::new());
        }
        let schema = Arc::new(Schema::new(
            raw_schema.fields().iter()
                .map(|f| Arc::new(Field::new(f.name(), DataType::Utf8, true)))
                .collect::<Vec<_>>(),
        ));

        let mut builder = ColumnStatsBuilder::new(col_count, page_row_starts);

        let format = arrow::csv::reader::Format::default()
            .with_header(true)
            .with_delimiter(delimiter);
        let reader = arrow::csv::ReaderBuilder::new(schema)
            .with_format(format)
            .with_batch_size(8192)
            .build(std::io::Cursor::new(bytes))
            .map_err(|e| BundlebaseError::from(format!("CSV stats read error: {}", e)))?;

        for batch_result in reader {
            let batch = batch_result
                .map_err(|e| BundlebaseError::from(format!("CSV stats batch error: {}", e)))?;
            builder.process_batch(&batch);
        }

        Ok(builder.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::ReaderPlugin;
    use crate::test_utils::{test_datafile, test_context};
    use arrow::array::{downcast_array, Array, StringArray};
    use futures::stream::StreamExt;

    #[tokio::test]
    async fn test_wrong_format() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let binding = test_context();
        let result = plugin
            .reader("file:///test.parquet", &BlockId::generate(), &binding, None, None, None, None)
            .await?;

        assert!(result.is_none(), "CsvPlugin should reject non-Csv format");

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_csv_file() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let binding = test_context();
        let invalid_reader = plugin
            .reader("file:///invalid.csv", &BlockId::generate(), &binding, None, None, None, None)
            .await?;

        assert!(invalid_reader.is_some());

        // Schema access should fail for nonexistent file
        let schema_result = invalid_reader.expect("checked above").read_schema().await;
        assert!(
            schema_result.is_err(),
            "Schema access should fail for nonexistent file"
        );

        Ok(())
    }

    #[tokio::test]
    async fn read() -> Result<(), BundlebaseError> {
        // Test complete CSV file read and data validation
        let plugin = CsvPlugin::default();

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("customers-0-100.csv"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Expected column names from customers-0-100.csv
        let column_names = vec![
            "Index",
            "Customer Id",
            "First Name",
            "Last Name",
            "Company",
            "City",
            "Country",
            "Phone 1",
            "Phone 2",
            "Email",
            "Subscription Date",
            "Website",
        ];

        // Validate schema
        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let actual_columns: Vec<_> = schema.fields().iter().map(|f| f.name().clone()).collect();

        assert_eq!(
            column_names, actual_columns,
            "CSV schema should match expected columns"
        );

        // Validate data reading
        let reader = plugin
            .reader(
                test_datafile("customers-0-100.csv"),
                &BlockId::generate(),
                &binding,
                Some(schema),
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let binding2 = test_context();
        let ctx = &binding2.ctx;
        let ds = reader.data_source(None, &[], None, None).await?;
        let results = ds.open(0, ctx.task_ctx())?;

        let result_columns: Vec<_> = results
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();

        assert_eq!(
            column_names, result_columns,
            "Data source schema should match expected columns"
        );

        // Validate actual data
        let batches = results.collect::<Vec<_>>().await;
        assert_eq!(1, batches.len(), "Should have one record batch");

        let row1 = batches[0]
            .as_ref()
            .map_err(|e| BundlebaseError::from(e.to_string()))?;

        // Validate "First Name" column (index 2)
        assert_eq!(
            "Utf8",
            row1.column(2).data_type().to_string(),
            "First Name should be Utf8 type"
        );

        let name_array: StringArray = downcast_array(row1.column(2).as_ref());
        assert_eq!("Sheryl", name_array.value(0), "First name should be Sheryl");
        assert_eq!(
            "Preston",
            name_array.value(1),
            "Second name should be Preston"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_schema_all_utf8() -> Result<(), BundlebaseError> {
        // CSV columns should always be Utf8 — no type inference
        let plugin = CsvPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("customers-0-100.csv"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let schema = reader.read_schema().await?.ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        for field in schema.fields() {
            assert_eq!(
                field.data_type(),
                &arrow::datatypes::DataType::Utf8,
                "Field '{}' should be Utf8 but was {:?}",
                field.name(),
                field.data_type()
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_statistics() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("customers-0-100.csv"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Statistics should be available for a valid CSV file
        let stats = reader.read_statistics().await?;
        assert!(
            stats.is_some(),
            "Statistics should be available for CSV file"
        );

        let stats = stats.expect("checked above");

        // Extract actual row count from statistics
        let rows = stats.num_rows.get_value().ok_or_else(|| BundlebaseError::from("Expected row count"))?;

        // Now CSV statistics should return the actual row count by reading the file
        // customers-0-100.csv has 100 data rows (plus 1 header row)
        assert_eq!(
            &100, rows,
            "CSV statistics should return actual row count from file. Got {} rows",
            rows
        );

        // Extract the byte size from statistics
        let bytes = match stats.total_byte_size {
            Precision::Exact(n) | Precision::Inexact(n) => n,
            _ => 0,
        };

        // customers-0-100.csv is 17160 bytes
        assert_eq!(
            17160, bytes,
            "CSV statistics should return correct file size in bytes. Got {} bytes",
            bytes
        );

        Ok(())
    }

    /// Test that CSV files with newlines inside quoted values are auto-detected
    /// and schema inference succeeds after retry with newlines_in_values=true.
    #[tokio::test]
    async fn test_newlines_in_values_schema() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("newlines-in-values.csv"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Schema inference should succeed (retry with newlines_in_values=true)
        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let column_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            column_names,
            vec!["id", "name", "description", "value"],
            "Schema should have the correct columns"
        );

        Ok(())
    }

    /// A regular CSV should not set newlines_in_values.
    #[tokio::test]
    async fn test_regular_csv_read_options_empty() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("customers-0-100.csv"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Schema inference should succeed without retry
        let _schema = reader.read_schema().await?;

        // read_options should be empty for regular CSV
        let options = reader.read_options();
        assert!(
            options.is_empty(),
            "Regular CSV should not have read_options, got: {:?}",
            options
        );

        Ok(())
    }

    /// When read_options with newlines_in_values=true are passed to the reader,
    /// schema inference should succeed on the first try (no retry needed).
    #[tokio::test]
    async fn test_newlines_in_values_with_stored_options() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let mut options = HashMap::new();
        options.insert("newlines_in_values".to_string(), "true".to_string());

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("newlines-in-values.csv"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                Some(&options),
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Should succeed on first try since options are pre-configured
        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let column_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            column_names,
            vec!["id", "name", "description", "value"],
            "Schema should have the correct columns with stored options"
        );

        Ok(())
    }

    /// Test reading data from a CSV with newlines in values.
    #[tokio::test]
    async fn test_newlines_in_values_data() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let binding = test_context();
        // First read schema (triggers auto-detection)
        let reader = plugin
            .reader(
                test_datafile("newlines-in-values.csv"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let schema = reader.read_schema().await?.ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        // Create a new reader with the detected options for data reading
        let options = reader.read_options();
        let reader = plugin
            .reader(
                test_datafile("newlines-in-values.csv"),
                &BlockId::generate(),
                &binding,
                Some(schema),
                None,
                None,
                Some(&options),
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let binding2 = test_context();
        let ctx = &binding2.ctx;
        let ds = reader.data_source(None, &[], None, None).await?;
        let results = ds.open(0, ctx.task_ctx())?;
        let batches = results.collect::<Vec<_>>().await;

        assert_eq!(1, batches.len(), "Should have one record batch");

        let batch = batches[0]
            .as_ref()
            .map_err(|e| BundlebaseError::from(e.to_string()))?;

        assert_eq!(
            3,
            batch.num_rows(),
            "Should have 3 data rows (newlines in values don't create extra rows)"
        );

        Ok(())
    }

    /// Test that CSV files with backslash-before-quote patterns can be read.
    /// The backslash-in-values.csv has `\"` patterns that confuse object_store's
    /// LineDelimiter. Schema inference should succeed (either directly or after
    /// retry with newlines_in_values=true).
    #[tokio::test]
    async fn test_backslash_in_values_schema() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("backslash-in-values.csv"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let column_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            column_names,
            vec!["id", "category", "question", "answer"],
            "Schema should have the correct columns"
        );

        Ok(())
    }

    /// Test that reproduces the LineDelimiter error by reading a CSV from the
    /// local filesystem, where the object store streams data in chunks.
    /// The LineDelimiter in object_store treats `\` as escape, which conflicts
    /// with CSV data that doesn't use backslash escaping.
    #[tokio::test]
    async fn test_filesystem_csv_with_jeopardy_pattern() -> Result<(), BundlebaseError> {
        use std::io::Write;

        // Create a temp file with Jeopardy-style CSV patterns.
        // The key is: the file must be large enough that object_store streams
        // it in multiple chunks (default 4KB), AND the data has `""` patterns
        // throughout. 5000 rows (~500KB) is sufficient to trigger chunked reads.
        let temp_dir = tempfile::tempdir()?;
        let csv_path = temp_dir.path().join("jeopardy_test.csv");

        {
            let mut f = std::fs::File::create(&csv_path)?;
            writeln!(f, "show_num,air_date,round,category,value,question,answer")?;
            for i in 0..5000 {
                writeln!(
                    f,
                    "{},2024-01-01,Jeopardy!,\"CATEGORY {}\",\"$200\",\"In 1963, live on \"\"The Art Linkletter Show\"\", this company served its billionth burger\",\"McDonald's\"",
                    i, i
                )?;
            }
        }

        let csv_url = format!("file://{}", csv_path.to_str().expect("valid path"));
        let plugin = CsvPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(&csv_url, &BlockId::generate(), &binding, None, None, None, None)
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // This should succeed — either directly or after CsvReader's retry logic
        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let column_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            column_names,
            vec![
                "show_num", "air_date", "round", "category", "value", "question",
                "answer"
            ],
        );

        Ok(())
    }

    /// Test that whitespace is trimmed from CSV column names during schema inference.
    #[tokio::test]
    async fn test_whitespace_trimmed_from_column_names() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("whitespace-headers.csv"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let column_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            column_names,
            vec!["Id", "Name", "Value", "Category"],
            "Column names should have whitespace trimmed"
        );

        Ok(())
    }

    /// Verify the is_line_delimiter_error detection matches the actual error format.
    #[tokio::test]
    async fn test_is_line_delimiter_error_detection() {
        // Construct an error matching the real format from object_store's LineDelimiter
        let inner: BundlebaseError =
            "Object Store error: Generic LineDelimiter error: encountered unterminated string"
                .to_string()
                .into();
        assert!(
            is_line_delimiter_error(&inner),
            "Should detect LineDelimiter unterminated string error"
        );

        // Non-matching errors should not trigger retry
        let other: BundlebaseError = "Some other error".to_string().into();
        assert!(
            !is_line_delimiter_error(&other),
            "Should not match unrelated errors"
        );
    }

    /// Test with an in-memory CSV that has double-double-quote patterns.
    /// Verifies schema inference works correctly with standard CSV quoting.
    #[tokio::test]
    async fn test_double_quote_escape_csv() -> Result<(), BundlebaseError> {
        use bundlebase_io::file::IOReadWriteFile;
        use bundlebase_io::plugin::object_store::ObjectStoreFile;

        let mut csv = String::from("id,category,question,answer\n");
        for i in 0..1000 {
            csv.push_str(&format!(
                "{},\"CAT\",\"He said \"\"hello\"\" and \"\"goodbye\"\"\",\"answer {}\"\n",
                i, i
            ));
        }

        let url = Url::parse("memory:///test_dblquote_csv/data.csv")?;
        let file = ObjectStoreFile::from_url(&url, crate::test_utils::test_config())?;
        file.write(bytes::Bytes::from(csv.into_bytes())).await?;

        let plugin = CsvPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(url.as_str(), &BlockId::generate(), &binding, None, None, None, None)
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let column_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            column_names,
            vec!["id", "category", "question", "answer"],
        );

        Ok(())
    }

    /// Test that reads the actual jeopardy CSV file if it exists on disk.
    /// This file triggers the LineDelimiter "unterminated string" error,
    /// which should be caught by our retry logic.
    /// This test is ignored by default since it requires a specific data file.
    #[tokio::test]
    async fn test_jeopardy_csv_from_disk() -> Result<(), BundlebaseError> {
        let csv_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../bundlebase/../../datasets/jeoparady/9b/da082ca1451f90.csv"
        );

        if !std::path::Path::new(csv_path).exists() {
            return Ok(());
        }

        // First, verify the error actually occurs without our retry logic
        // by using the raw FileReader directly
        let csv_url_str = format!("file://{}", csv_path);
        let _csv_url = Url::parse(&csv_url_str)?;
        let config = CsvFormatConfig::default();
        let binding = test_context();
        let file = crate::plugin::file_reader::FilePlugin::new(config.clone())
            .reader(&csv_url_str, &binding, None, None)
            .await?;

        // The raw FileReader should fail with the LineDelimiter error
        let raw_result = file.read_schema().await;
        assert!(
            raw_result.is_err(),
            "Raw FileReader should fail on jeopardy CSV; this file triggers the LineDelimiter bug"
        );
        let raw_err = raw_result.err().expect("checked above");
        let raw_msg = raw_err.to_string();
        assert!(
            raw_msg.contains("LineDelimiter") && raw_msg.contains("unterminated string"),
            "Error should be a LineDelimiter error, got: {}",
            raw_msg
        );

        // Now test with CsvReader which has the retry logic
        let plugin = CsvPlugin::default();
        let reader = plugin
            .reader(&csv_url_str, &BlockId::generate(), &binding, None, None, None, None)
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // CsvReader's retry should catch the error and succeed
        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Show Number",
                "Air Date",
                "Round",
                "Category",
                "Value",
                "Question",
                "Answer"
            ]
        );

        Ok(())
    }
}
