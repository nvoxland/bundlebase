use crate::DataContext;
use crate::plugin::file_reader::{FileFormatConfig, FilePlugin, FileReader};
use crate::plugin::ReaderPlugin;
use crate::{BlockId, DataReader, LineOrientedFormat, PageMapDataSource, RowId};
use crate::page_map::{PageMap, resolve_row_numbers_to_byte_offsets};
use bundlebase_io::plugin::object_store::ObjectStoreFile;
use bundlebase_io::IOReadWriteDir;
use bundlebase_common::BundlebaseError;
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::common::stats::Precision;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::file_format::json::JsonFormat;
use datafusion::datasource::file_format::FileFormat;
use datafusion::datasource::physical_plan::{FileSource, JsonSource};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
use futures::stream::StreamExt;
use std::sync::Arc;
use url::Url;

/// Configuration for JSON format
#[derive(Debug, Clone, Default)]
pub struct JsonlFormatConfig {
    /// When true, nested objects and arrays stored in string columns are
    /// re-serialized into canonical JSON (keys sorted, whitespace stripped)
    /// instead of being kept verbatim. Source option: `normalize_nested_json`.
    pub normalize_nested_json: bool,
}

impl JsonlFormatConfig {
    pub fn from_read_options(opts: &std::collections::HashMap<String, String>) -> Self {
        Self {
            normalize_nested_json: opts
                .get("normalize_nested_json")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }
}

impl FileFormatConfig for JsonlFormatConfig {
    fn extensions(&self) -> &'static [&'static str] {
        &[".json", ".jsonl"]
    }

    fn file_format(&self) -> Arc<dyn FileFormat> {
        Arc::new(JsonFormat::default())
    }

    fn file_source(&self, schema: SchemaRef) -> Arc<dyn FileSource> {
        Arc::new(JsonSource::new(schema))
    }

    fn line_oriented_format(&self) -> Option<LineOrientedFormat> {
        Some(LineOrientedFormat::JsonLines)
    }
}

/// JSON plugin - uses generic FilePlugin and creates JsonlReader
#[derive(Default)]
pub struct JsonlPlugin {
    config: JsonlFormatConfig,
}

#[async_trait]
impl ReaderPlugin for JsonlPlugin {
    async fn reader(
        &self,
        source: &str,
        block_id: &BlockId,
        bundle: &dyn DataContext,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
        read_options: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError> {
        let lower = source.to_lowercase();
        if !lower.ends_with(".json") && !lower.ends_with(".jsonl") {
            return Ok(None);
        }

        let config = match read_options {
            Some(opts) if !opts.is_empty() => JsonlFormatConfig::from_read_options(opts),
            _ => self.config.clone(),
        };
        let plugin = FilePlugin::new(config);

        let layout = match layout {
            None => None,
            Some(x) => Some(ObjectStoreFile::from_str(
                x.as_str(),
                bundle.data_context_dir().as_ref(),
                bundle.config_provider(),
            )?),
        };

        let reader = plugin
            .reader(source, bundle, schema, expected_version)
            .await?;
        Ok(Some(Arc::new(JsonlReader::new(reader, block_id, &layout))))
    }
}

#[derive(Debug)]
pub struct JsonlReader {
    inner: FileReader<JsonlFormatConfig>,
    block_id: BlockId,
    layout: Option<ObjectStoreFile>,
}

impl JsonlReader {
    pub fn new(
        inner: FileReader<JsonlFormatConfig>,
        block_id: &BlockId,
        layout: &Option<ObjectStoreFile>,
    ) -> Self {
        Self {
            inner,
            block_id: *block_id,
            layout: layout.clone(),
        }
    }
}

#[async_trait]
impl DataReader for JsonlReader {
    fn url(&self) -> &Url {
        self.inner.url()
    }

    fn block_id(&self) -> BlockId {
        self.block_id
    }

    fn format(&self) -> crate::attach_format::AttachFormat {
        crate::attach_format::AttachFormat::JsonL
    }

    fn read_options(&self) -> std::collections::HashMap<String, String> {
        let mut opts = std::collections::HashMap::new();
        if self.inner.config().normalize_nested_json {
            opts.insert("normalize_nested_json".to_string(), "true".to_string());
        }
        opts
    }

    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError> {
        // Query-time fast path: if a schema was persisted at attach time and
        // handed to us via the inner FileReader, return it directly. Avoids
        // re-fetching the file and re-parsing lines on every query.
        if let Some(cached) = self.inner.schema() {
            return Ok(Some(cached.clone()));
        }

        // Attach-time path: read the first 64 KB, check it's not a JSON
        // array, and infer field names from the first line.
        let store = self.inner.object_store();
        let path = object_store::path::Path::parse(self.inner.url().path())?;
        let opts = object_store::GetOptions {
            range: Some((0..65536).into()),
            ..Default::default()
        };
        let result = store.get_opts(&path, opts).await?;
        let bytes = result.bytes().await?;

        // Validate the file is JSONL (one object per line), not a JSON array.
        let first_char = bytes.iter()
            .find(|b| !b.is_ascii_whitespace())
            .copied();
        if first_char == Some(b'[') {
            return Err(BundlebaseError::from(format!(
                "File '{}' contains a JSON array, not JSON Lines. \
                 Use a connector with SAVE AS PARQUET to convert JSON arrays, \
                 or convert the file to JSONL format (one JSON object per line).",
                self.inner.url()
            )));
        }

        // Read field names from the first JSON line and treat all columns as
        // Utf8, just like CSV. Earlier we tried unioning field names across
        // every line of the file so the query-time visitor could skip the
        // `serde_json::Deserializer::ignore_value` path on unknown keys, but
        // that doubled column counts on heterogeneous datasets and the extra
        // per-row "fill missing with empty" work more than ate the savings.
        let first_line = bytes.iter()
            .position(|&b| b == b'\n')
            .map(|pos| &bytes[..pos])
            .unwrap_or(&bytes[..]);
        let parsed: serde_json::Value = serde_json::from_slice(first_line)
            .map_err(|e| BundlebaseError::from(format!("Failed to parse first JSONL line from {}: {}", self.inner.url(), e)))?;
        if let serde_json::Value::Object(map) = parsed {
            let fields: Vec<arrow::datatypes::Field> = map.keys()
                .map(|k| arrow::datatypes::Field::new(k, arrow::datatypes::DataType::Utf8, true))
                .collect();
            Ok(Some(Arc::new(arrow::datatypes::Schema::new(fields))))
        } else {
            Err(BundlebaseError::from(format!("First JSONL line is not a JSON object in {}", self.inner.url())))
        }
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
                false, // no header in JSON Lines
            )
            .await
            .map_err(|e| DataFusionError::External(e))?;

            let schema = self
                .read_schema()
                .await
                .map_err(|e| DataFusionError::External(e))?
                .ok_or_else(|| DataFusionError::Internal("No schema available".to_string()))?;

            return Ok(Arc::new(PageMapDataSource::new(
                self.inner.file().as_object_store_file(),
                schema,
                byte_offsets,
                projection.cloned(),
                LineOrientedFormat::JsonLines,
                self.inner.config().normalize_nested_json,
            )));
        }
        // Read JSONL with serde_json and stringify all values to produce all-Utf8
        // Arrow columns. This avoids Arrow's strict JSON reader which rejects
        // non-string values (booleans, numbers, objects) when schema says Utf8.
        let schema = self
            .read_schema()
            .await
            .map_err(|e| DataFusionError::External(e))?
            .ok_or_else(|| DataFusionError::Internal("No schema available".to_string()))?;

        let store = self.inner.object_store();
        let path = object_store::path::Path::parse(self.inner.url().path())
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        let result = store.get_opts(&path, Default::default()).await
            .map_err(|e| DataFusionError::External(Box::new(e)))?;
        let bytes = result.bytes().await
            .map_err(|e| DataFusionError::External(Box::new(e)))?;

        let col_names: Vec<&str> =
            schema.fields().iter().map(|f| f.name().as_str()).collect();
        let name_to_idx: std::collections::HashMap<&str, usize> =
            col_names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let mut builders: Vec<arrow::array::StringBuilder> = (0..col_names.len())
            .map(|_| arrow::array::StringBuilder::new())
            .collect();
        let normalize = self.inner.config().normalize_nested_json;

        for line in bytes.split(|&b| b == b'\n') {
            let line = if line.last() == Some(&b'\r') { &line[..line.len() - 1] } else { line };
            if line.is_empty() { continue; }
            crate::jsonl_row::append_jsonl_row_to_builders(line, &name_to_idx, &mut builders, normalize);
        }

        let arrays: Vec<Arc<dyn arrow::array::Array>> = builders
            .into_iter()
            .map(|mut b| Arc::new(b.finish()) as Arc<dyn arrow::array::Array>)
            .collect();
        let batch = arrow::record_batch::RecordBatch::try_new(schema.clone(), arrays)
            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;

        let proj = projection.cloned();
        let partitions = vec![vec![batch]];
        let source = Arc::new(
            datafusion::datasource::memory::MemorySourceConfig::try_new(&partitions, schema, proj)?
        );
        Ok(source as Arc<dyn DataSource>)
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

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Box::new(e) as BundlebaseError)?;
            for &b in chunk.iter() {
                if b == b'\n' {
                    row_count += 1;
                }
            }
            file_size += chunk.len();
            if !chunk.is_empty() {
                last_byte = chunk[chunk.len() - 1];
            }
        }

        // Account for final line without trailing newline
        if file_size > 0 && last_byte != b'\n' {
            row_count += 1;
        }

        let stats = Statistics {
            num_rows: Precision::Exact(row_count),
            total_byte_size: Precision::Exact(file_size),
            ..Default::default()
        };

        Ok(Some(stats))
    }

    async fn column_stats(&self) -> Result<Vec<crate::page_map::ColumnStats>, BundlebaseError> {
        use crate::layout_cache::GLOBAL_LAYOUT_CACHE;
        use bundlebase_io::IOReadFile;

        let layout_file = match &self.layout {
            Some(f) => f,
            None => return Ok(vec![]),
        };
        let layout_url = layout_file.url().clone();
        let layout = if let Some(cached) = GLOBAL_LAYOUT_CACHE.get(&layout_url) {
            cached
        } else {
            let loaded = PageMap::load(layout_file).await?;
            let arc = Arc::new(loaded);
            GLOBAL_LAYOUT_CACHE.insert(layout_url, arc.clone());
            arc
        };
        Ok(layout.column_stats.clone())
    }

    async fn build_layout(
        &self,
        data_dir: &dyn IOReadWriteDir,
    ) -> Result<Option<Box<dyn bundlebase_io::IOReadFile>>, BundlebaseError> {
        use crate::column_stats_builder::ColumnStatsBuilder;
        use crate::page_map::DEFAULT_PAGE_SIZE;
        use futures::stream;
        use object_store::GetOptions;
        use std::collections::HashMap;

        let object_store = self.inner.object_store();
        let path = object_store::path::Path::parse(self.inner.url().path())?;

        // Get column names from the schema for name→idx mapping
        // Use self.read_schema() (not self.inner) to get the fallback-aware schema
        let schema = match self.read_schema().await? {
            Some(s) => s,
            None => return Ok(None),
        };
        let col_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        let col_count = col_names.len();
        let name_to_idx: HashMap<&str, usize> = col_names.iter().enumerate().map(|(i, n)| (*n, i)).collect();

        // Read file bytes once — used for both page layout and column stats.
        let get_result = object_store.get_opts(&path, GetOptions::default()).await?;
        let mut file_stream = get_result.into_stream();
        let mut buffer = Vec::new();
        while let Some(chunk) = file_stream.next().await {
            let chunk = chunk.map_err(|e| Box::new(e) as BundlebaseError)?;
            buffer.extend_from_slice(&chunk);
        }

        // Build page layout first (no stats yet) to get page boundaries.
        let initial_layout = match PageMap::build(&buffer, false, DEFAULT_PAGE_SIZE, vec![]) {
            None => return Ok(None),
            Some(l) => l,
        };
        let page_row_starts: Vec<u32> = initial_layout.pages.iter().map(|p| p.row_begin).collect();

        // Compute column stats with page-level tracking.
        let mut builder = if col_count > 0 { ColumnStatsBuilder::new(col_count, &page_row_starts) } else { ColumnStatsBuilder::new(0, &[]) };

        for line in buffer.split(|&b| b == b'\n') {
            let line = if line.last() == Some(&b'\r') { &line[..line.len() - 1] } else { line };
            if line.is_empty() { continue; }
            match serde_json::from_slice::<serde_json::Value>(line) {
                Ok(serde_json::Value::Object(m)) => {
                    builder.process_jsonl_row(&m, &name_to_idx, col_count);
                }
                _ => continue,
            }
        }

        let column_stats = builder.finish();

        // Assemble final layout with stats and write.
        let layout = PageMap { column_stats, ..initial_layout };
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
        use crate::page_map::PageMap;
        use bundlebase_index::{FilterAnalyzer, IndexPredicate};

        let layout_file = match &self.layout {
            Some(f) => f,
            None => return Ok(None),
        };

        let layout_url = layout_file.url().clone();
        let layout = if let Some(cached) = GLOBAL_LAYOUT_CACHE.get(&layout_url) {
            cached
        } else {
            let loaded = PageMap::load(layout_file).await?;
            let arc = Arc::new(loaded);
            GLOBAL_LAYOUT_CACHE.insert(layout_url, arc.clone());
            arc
        };

        if layout.pages.len() <= 1 {
            return Ok(None);
        }

        // Use JsonlReader::read_schema (not FileReader::read_schema) so we
        // go through the serde_json-based all-Utf8 fallback and avoid
        // Arrow's JsonFormat::infer_schema, which errors on JSONL files
        // whose columns change type between records.
        let schema = match self.read_schema().await? {
            Some(s) => s,
            None => return Ok(None),
        };

        let indexable = FilterAnalyzer::extract_indexable(filters);
        if indexable.is_empty() {
            return Ok(None);
        }

        let num_pages = layout.pages.len();
        let mut include = vec![true; num_pages];

        for filter in &indexable {
            let col_idx = match schema.fields().iter().position(|f| f.name() == &filter.column) {
                Some(i) => i,
                None => continue,
            };
            let col_stats = match layout.column_stats.get(col_idx) {
                Some(s) => s,
                None => continue,
            };
            if col_stats.page_stats.is_empty() { continue; }

            let is_increasing = col_stats.is_strictly_increasing;
            let is_decreasing = col_stats.is_strictly_decreasing;

            for (page_idx, page_stat) in col_stats.page_stats.iter().enumerate() {
                if !include[page_idx] { continue; }
                let bloom = page_stat.bloom_filter.as_deref();
                let can_prune = match &filter.predicate {
                    IndexPredicate::Exact(val) => prune_exact_with_bloom(val, page_stat.min.as_ref(), page_stat.max.as_ref(), bloom),
                    IndexPredicate::Range { min: fmin, max: fmax } => prune_range(fmin, fmax, page_stat.min.as_ref(), page_stat.max.as_ref()),
                    IndexPredicate::In(vals) => vals.iter().all(|v| prune_exact_with_bloom(v, page_stat.min.as_ref(), page_stat.max.as_ref(), bloom)),
                    // Per-page null counts not tracked; null pruning only works at block level
                    IndexPredicate::IsNull | IndexPredicate::IsNotNull => false,
                    IndexPredicate::Prefix(prefix) => prune_prefix(prefix, page_stat.min.as_ref(), page_stat.max.as_ref()),
                };
                if can_prune { include[page_idx] = false; }
            }

            if is_increasing {
                if let Some(upper) = extract_upper_bound(&filter.predicate) {
                    let mut past_range = false;
                    for (page_idx, page_stat) in col_stats.page_stats.iter().enumerate() {
                        if past_range { include[page_idx] = false; }
                        else if let Some(ref pmin) = page_stat.min {
                            if is_value_above_bound(&upper, pmin) {
                                include[page_idx] = false;
                                past_range = true;
                            }
                        }
                    }
                }
            }
            if is_decreasing {
                if let Some(lower) = extract_lower_bound(&filter.predicate) {
                    let mut past_range = false;
                    for (page_idx, page_stat) in col_stats.page_stats.iter().enumerate() {
                        if past_range { include[page_idx] = false; }
                        else if let Some(ref pmax) = page_stat.max {
                            if is_value_below_bound(&lower, pmax) {
                                include[page_idx] = false;
                                past_range = true;
                            }
                        }
                    }
                }
            }
        }

        if include.iter().all(|&b| b) {
            return Ok(None);
        }

        let included: Vec<usize> = include.iter().enumerate()
            .filter(|(_, &inc)| inc)
            .map(|(i, _)| i)
            .collect();

        if included.is_empty() {
            return Ok(None);
        }

        // Coalesce adjacent / nearby pages into single range reads (256KB gap threshold).
        const GAP_THRESHOLD: u64 = 256 * 1024;
        let page_ranges = crate::page_map_data_source::coalesce_page_ranges(
            &layout.pages,
            &included,
            layout.file_size,
            GAP_THRESHOLD,
        );

        log::debug!(
            "JSONL page-filter: {} included pages → {} coalesced ranges",
            included.len(),
            page_ranges.len()
        );

        Ok(Some(Arc::new(PageMapDataSource::from_page_ranges(
            self.inner.file().as_object_store_file(),
            schema,
            page_ranges,
            projection.cloned(),
            LineOrientedFormat::JsonLines,
            self.inner.config().normalize_nested_json,
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::ReaderPlugin;
    use crate::test_utils::{test_datafile, test_context};
    use arrow::array::{downcast_array, Array, StringArray};
    use datafusion::common::stats::Precision;
    use futures::stream::StreamExt;

    #[tokio::test]
    async fn test_wrong_file_extension() -> Result<(), BundlebaseError> {
        // JSON plugin should only adapt .json/.jsonl files
        let plugin = JsonlPlugin::default();

        let binding = test_context();
        let result = plugin
            .reader("file:///test.csv", &BlockId::generate(), &binding, None, None, None, None)
            .await?;

        assert!(result.is_none(), "JsonlPlugin should reject non-JsonL format");

        Ok(())
    }

    #[tokio::test]
    async fn test_handles_jsonl_extension() -> Result<(), BundlebaseError> {
        let plugin = JsonlPlugin::default();

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("objects.jsonl"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader for .jsonl file"))?;

        // Validate schema works for .jsonl files
        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let actual_columns: Vec<_> = schema.fields().iter().map(|f| f.name().clone()).collect();
        assert_eq!(
            vec!["completed", "name", "score", "session"],
            actual_columns,
            "JSONL schema should match expected columns"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_json_file() -> Result<(), BundlebaseError> {
        let plugin = JsonlPlugin::default();

        let binding = test_context();
        let invalid_reader = plugin
            .reader("file:///invalid.jsonl", &BlockId::generate(), &binding, None, None, None, None)
            .await?;

        assert!(
            invalid_reader.is_some(),
            "Plugin should return reader for .jsonl URL even if file doesn't exist"
        );

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
        // Test complete JSON file read and data validation
        let plugin = JsonlPlugin::default();

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("objects.jsonl"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Expected column names from objects.jsonl
        let column_names = vec!["completed", "name", "score", "session"];

        // Validate schema
        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let actual_columns: Vec<_> = schema.fields().iter().map(|f| f.name().clone()).collect();

        assert_eq!(
            column_names, actual_columns,
            "JSON schema should match expected columns"
        );

        // Validate data reading
        let reader = plugin
            .reader(
                test_datafile("objects.jsonl"),
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

        // Validate "name" column (index 1)
        assert_eq!(
            "Utf8",
            row1.column(1).data_type().to_string(),
            "name column should be Utf8 type"
        );

        let name_array: StringArray = downcast_array(row1.column(1).as_ref());
        assert_eq!(
            "Gilbert",
            name_array.value(0),
            "First name should be Gilbert"
        );
        assert_eq!("Alexa", name_array.value(1), "Second name should be Alexa");

        Ok(())
    }

    #[tokio::test]
    async fn test_statistics() -> Result<(), BundlebaseError> {
        let plugin = JsonlPlugin::default();

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("objects.jsonl"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Statistics should be available for a valid JSON file
        let stats = reader.read_statistics().await?.ok_or_else(|| BundlebaseError::from("Expected stats"))?;

        // Extract the row count from statistics
        let rows = stats.num_rows.get_value().ok_or_else(|| BundlebaseError::from("Expected row count"))?;

        // Now JSON statistics should return the actual row count by reading the file
        // objects.jsonl has 4 JSON objects (4 lines in JSONL format)
        assert_eq!(
            &4, rows,
            "JSON statistics should return actual row count from file. Got {} rows",
            rows
        );

        // Extract the byte size from statistics
        let bytes = match stats.total_byte_size {
            Precision::Exact(n) | Precision::Inexact(n) => n,
            _ => 0,
        };

        // objects.jsonl is 280 bytes
        assert_eq!(
            280, bytes,
            "JSON statistics should return correct file size in bytes. Got {} bytes",
            bytes
        );

        Ok(())
    }
}
