use crate::page_map::{resolve_row_numbers_to_byte_offsets, PageMap};
use crate::plugin::file_reader::{FileFormatConfig, FilePlugin, FileReader};
use crate::plugin::ReaderPlugin;
use crate::DataContext;
use crate::{BlockId, DataReader, LineOrientedFormat, PageMapDataSource, RowId};
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use bundlebase_common::BundlebaseError;
use bundlebase_io::plugin::object_store::ObjectStoreFile;
use bundlebase_io::IOReadWriteDir;
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
pub struct JsonlFormatConfig;

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

        let _ = read_options;
        let plugin = FilePlugin::new(self.config.clone());

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
        std::collections::HashMap::new()
    }

    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError> {
        // Query-time fast path: if a schema was persisted at attach time and
        // handed to us via the inner FileReader, return it directly. Avoids
        // re-fetching the file and re-parsing lines on every query.
        if let Some(cached) = self.inner.schema() {
            return Ok(Some(cached.clone()));
        }

        // Attach-time path: stream the full file and union top-level keys
        // across every line. Heterogeneous JSONL datasets (e.g. claude
        // transcripts where a `summary` record on line 1 has different keys
        // than the `user`/`assistant` rows that follow) would otherwise
        // silently drop the fields that aren't on line 1. Correctness wins
        // over the per-row cost of a wider schema. Lines are processed as
        // chunks arrive — the file is never buffered whole (bb-xn0.2).
        let store = self.inner.object_store();
        let path = object_store::path::Path::parse(self.inner.url().path())?;
        let result = store
            .get_opts(&path, object_store::GetOptions::default())
            .await?;
        let mut stream = result.into_stream();

        let mut keys: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut saw_object = false;
        let mut saw_non_ws = false;
        let mut pending: Vec<u8> = Vec::new();

        fn collect_line_keys(
            line: &[u8],
            keys: &mut Vec<String>,
            seen: &mut std::collections::HashSet<String>,
            saw_object: &mut bool,
        ) {
            let line = if line.last() == Some(&b'\r') {
                &line[..line.len() - 1]
            } else {
                line
            };
            if line.iter().all(|b| b.is_ascii_whitespace()) {
                return;
            }
            let parsed: serde_json::Value = match serde_json::from_slice(line) {
                Ok(v) => v,
                Err(_) => return, // schema inference is lenient; query-time scan errors on malformed lines
            };
            if let serde_json::Value::Object(map) = parsed {
                *saw_object = true;
                for k in map.keys() {
                    if !seen.contains(k) {
                        seen.insert(k.clone());
                        keys.push(k.clone());
                    }
                }
            }
        }

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Box::new(e) as BundlebaseError)?;
            // Validate the file is JSONL (one object per line), not a JSON array.
            if !saw_non_ws {
                if let Some(first) = chunk.iter().find(|b| !b.is_ascii_whitespace()) {
                    saw_non_ws = true;
                    if *first == b'[' {
                        return Err(BundlebaseError::from(format!(
                            "File '{}' contains a JSON array, not JSON Lines. \
                             Use a connector with SAVE AS PARQUET to convert JSON arrays, \
                             or convert the file to JSONL format (one JSON object per line).",
                            self.inner.url()
                        )));
                    }
                }
            }
            pending.extend_from_slice(&chunk);
            // Cursor scan + single compaction — per-line drain is quadratic
            // when a backend delivers the file as one chunk.
            let mut consumed = 0usize;
            while let Some(nl_rel) = memchr::memchr(b'\n', &pending[consumed..]) {
                let line_end = consumed + nl_rel;
                collect_line_keys(&pending[consumed..line_end], &mut keys, &mut seen, &mut saw_object);
                consumed = line_end + 1;
            }
            pending.drain(..consumed);
        }
        if !pending.is_empty() {
            collect_line_keys(&pending, &mut keys, &mut seen, &mut saw_object);
        }
        if !saw_object {
            return Err(BundlebaseError::from(format!(
                "No JSON objects found in {}",
                self.inner.url()
            )));
        }
        let fields: Vec<arrow::datatypes::Field> = keys
            .into_iter()
            .map(|k| arrow::datatypes::Field::new(k, arrow::datatypes::DataType::Utf8, true))
            .collect();
        Ok(Some(Arc::new(arrow::datatypes::Schema::new(fields))))
    }

    async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
        row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError> {
        if let Some(ids) = row_ids {
            // Match the full-scan path: a changed source file must error with
            // a version mismatch, not resolve byte offsets against new bytes.
            self.inner
                .file()
                .validate_version()
                .await
                .map_err(DataFusionError::External)?;

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
            )));
        }
        // Full scan: stream the file into bounded all-Utf8 batches. serde_json
        // stringification (not Arrow's strict JSON reader) tolerates
        // non-string values (booleans, numbers, objects) when schema says
        // Utf8; the streaming source never materializes the file (bb-xn0.2).
        let schema = self
            .read_schema()
            .await
            .map_err(|e| DataFusionError::External(e))?
            .ok_or_else(|| DataFusionError::Internal("No schema available".to_string()))?;

        let source = crate::streaming_jsonl_data_source::StreamingJsonlDataSource::new(
            self.inner.file().as_object_store_file(),
            schema,
            projection.cloned(),
        )?;
        Ok(Arc::new(source) as Arc<dyn DataSource>)
    }

    async fn read_version(&self) -> Result<String, BundlebaseError> {
        self.inner.version().await
    }

    async fn read_statistics(&self) -> Result<Option<Statistics>, BundlebaseError> {
        use crate::page_map::LogicalLineCounter;
        use object_store::GetOptions;

        let object_store = self.inner.object_store();
        let path = object_store::path::Path::parse(self.inner.url().path())?;

        // Stream the file counting logical rows — O(1) memory, no buffering.
        // Blank lines are skipped to match the parser's row numbering.
        let get_result = object_store.get_opts(&path, GetOptions::default()).await?;
        let mut stream = get_result.into_stream();

        let mut counter = LogicalLineCounter::new(false, b',');
        let mut file_size: usize = 0;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Box::new(e) as BundlebaseError)?;
            counter.update(&chunk);
            file_size += chunk.len();
        }

        let stats = Statistics {
            num_rows: Precision::Exact(counter.finish()),
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
        let name_to_idx: HashMap<&str, usize> =
            col_names.iter().enumerate().map(|(i, n)| (*n, i)).collect();

        // Pass 1: stream the file through the incremental page-map builder
        // to get page boundaries — the file is never buffered whole
        // (bb-xn0.2, ADR-003).
        let get_result = object_store.get_opts(&path, GetOptions::default()).await?;
        let mut file_stream = get_result.into_stream();
        let mut pm_builder = crate::page_map::PageMapBuilder::new(false, DEFAULT_PAGE_SIZE, false, b',');
        while let Some(chunk) = file_stream.next().await {
            let chunk = chunk.map_err(|e| Box::new(e) as BundlebaseError)?;
            pm_builder.feed(&chunk);
        }
        let initial_layout = match pm_builder.finish(vec![]) {
            None => return Ok(None),
            Some(l) => l,
        };
        let page_row_starts: Vec<u32> = initial_layout.pages.iter().map(|p| p.row_begin).collect();

        // Pass 2: stream the file again for column stats, now that page
        // boundaries are known. Two streaming passes keep memory O(1); the
        // second read costs I/O but attach-time layout building is one-shot.
        let mut builder = if col_count > 0 {
            ColumnStatsBuilder::new(col_count, &page_row_starts)
        } else {
            ColumnStatsBuilder::new(0, &[])
        };

        let mut process_stats_line = |line: &[u8]| {
            let line = if line.last() == Some(&b'\r') {
                &line[..line.len() - 1]
            } else {
                line
            };
            if line.is_empty() {
                return;
            }
            if let Ok(serde_json::Value::Object(m)) =
                serde_json::from_slice::<serde_json::Value>(line)
            {
                builder.process_jsonl_row(&m, &name_to_idx, col_count);
            }
        };

        let get_result = object_store.get_opts(&path, GetOptions::default()).await?;
        let mut file_stream = get_result.into_stream();
        let mut pending: Vec<u8> = Vec::new();
        while let Some(chunk) = file_stream.next().await {
            let chunk = chunk.map_err(|e| Box::new(e) as BundlebaseError)?;
            pending.extend_from_slice(&chunk);
            // Cursor scan + single compaction (see read_schema).
            let mut consumed = 0usize;
            while let Some(nl_rel) = memchr::memchr(b'\n', &pending[consumed..]) {
                let line_end = consumed + nl_rel;
                process_stats_line(&pending[consumed..line_end]);
                consumed = line_end + 1;
            }
            pending.drain(..consumed);
        }
        if !pending.is_empty() {
            process_stats_line(&pending.clone());
        }
        drop(process_stats_line);

        let column_stats = builder.finish(initial_layout.file_size);

        // Assemble final layout with stats and write.
        let layout = PageMap {
            column_stats,
            ..initial_layout
        };
        let index_bytes = layout.serialize()?;
        let data_stream = Box::pin(stream::once(
            async move { Ok::<_, std::io::Error>(index_bytes) },
        ));
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
        use crate::layout_cache::GLOBAL_LAYOUT_CACHE;
        use crate::page_filter::{
            extract_lower_bound, extract_upper_bound, is_value_above_bound, is_value_below_bound,
            prune_exact_with_bloom, prune_prefix, prune_range,
        };
        use crate::page_map::PageMap;
        use bundlebase_index::{FilterAnalyzer, IndexPredicate};
        use bundlebase_io::IOReadFile;

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
            let col_idx = match schema
                .fields()
                .iter()
                .position(|f| f.name() == &filter.column)
            {
                Some(i) => i,
                None => continue,
            };
            let col_stats = match layout.column_stats.get(col_idx) {
                Some(s) => s,
                None => continue,
            };
            if col_stats.page_stats.is_empty() {
                continue;
            }

            let is_increasing = col_stats.is_strictly_increasing;
            let is_decreasing = col_stats.is_strictly_decreasing;

            for (page_idx, page_stat) in col_stats.page_stats.iter().enumerate() {
                if !include[page_idx] {
                    continue;
                }
                let bloom = page_stat.bloom_filter.as_deref();
                let can_prune = match &filter.predicate {
                    IndexPredicate::Exact(val) => prune_exact_with_bloom(
                        val,
                        page_stat.min.as_ref(),
                        page_stat.max.as_ref(),
                        bloom,
                    ),
                    IndexPredicate::Range {
                        min: fmin,
                        max: fmax,
                    } => prune_range(fmin, fmax, page_stat.min.as_ref(), page_stat.max.as_ref()),
                    IndexPredicate::In(vals) => vals.iter().all(|v| {
                        prune_exact_with_bloom(
                            v,
                            page_stat.min.as_ref(),
                            page_stat.max.as_ref(),
                            bloom,
                        )
                    }),
                    // Per-page null counts not tracked; null pruning only works at block level
                    IndexPredicate::IsNull | IndexPredicate::IsNotNull => false,
                    IndexPredicate::Prefix(prefix) => {
                        prune_prefix(prefix, page_stat.min.as_ref(), page_stat.max.as_ref())
                    }
                };
                if can_prune {
                    include[page_idx] = false;
                }
            }

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

        if include.iter().all(|&b| b) {
            return Ok(None);
        }

        let included: Vec<usize> = include
            .iter()
            .enumerate()
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
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::ReaderPlugin;
    use crate::test_utils::{test_context, test_datafile};
    use arrow::array::{downcast_array, Array, StringArray};
    use datafusion::common::stats::Precision;
    use futures::stream::StreamExt;

    #[tokio::test]
    async fn test_wrong_file_extension() -> Result<(), BundlebaseError> {
        // JSON plugin should only adapt .json/.jsonl files
        let plugin = JsonlPlugin::default();

        let binding = test_context();
        let result = plugin
            .reader(
                "file:///test.csv",
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?;

        assert!(
            result.is_none(),
            "JsonlPlugin should reject non-JsonL format"
        );

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
            .reader(
                "file:///invalid.jsonl",
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
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

    /// Write JSONL content to a memory-store file and return the file handle.
    async fn write_jsonl(
        path: &str,
        content: &str,
    ) -> Result<ObjectStoreFile, BundlebaseError> {
        use bundlebase_io::file::IOReadWriteFile;
        let url = Url::parse(path)?;
        let file = ObjectStoreFile::from_url(&url, crate::test_utils::test_config())?;
        file.write(bytes::Bytes::from(content.as_bytes().to_vec()))
            .await?;
        Ok(file)
    }

    /// Read the given logical row numbers via the index (byte-offset) path and
    /// return the values of `column` in stream order.
    async fn read_rows_by_numbers(
        reader: &dyn DataReader,
        row_numbers: &[u32],
        column: &str,
    ) -> Result<Vec<String>, BundlebaseError> {
        let alias = crate::ObjectIdAlias::from(1u16);
        let row_ids: Vec<RowId> = row_numbers.iter().map(|&n| RowId::new(alias, n)).collect();
        let ds = reader.data_source(None, &[], None, Some(&row_ids)).await?;
        let ctx = test_context().ctx;
        let stream = ds.open(0, ctx.task_ctx())?;
        let batches: Vec<_> = stream.collect::<Vec<_>>().await;

        let mut values = Vec::new();
        for batch in batches {
            let batch = batch.map_err(|e| BundlebaseError::from(e.to_string()))?;
            if batch.num_rows() == 0 {
                continue;
            }
            let idx = batch.schema().index_of(column)?;
            let arr: StringArray = downcast_array(batch.column(idx).as_ref());
            for i in 0..arr.len() {
                values.push(arr.value(i).to_string());
            }
        }
        Ok(values)
    }

    /// Full scans must stream bounded batches, not materialize the whole
    /// file as one giant in-memory batch (bb-xn0.2, ADR-003).
    #[tokio::test]
    async fn test_full_scan_streams_in_bounded_batches() -> Result<(), BundlebaseError> {
        let mut content = String::with_capacity(1024 * 1024);
        for i in 0..20_000 {
            content.push_str(&format!("{{\"id\":\"{}\",\"name\":\"row{}\"}}\n", i, i));
        }
        write_jsonl("memory:///test_stream_scan/data.jsonl", &content).await?;

        let plugin = JsonlPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                "memory:///test_stream_scan/data.jsonl",
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let ds = reader.data_source(None, &[], None, None).await?;
        let ctx = test_context().ctx;
        let stream = ds.open(0, ctx.task_ctx())?;
        let batches: Vec<_> = stream.collect::<Vec<_>>().await;

        let mut total = 0usize;
        let mut first: Option<String> = None;
        let mut last: Option<String> = None;
        let n_batches = batches.len();
        for batch in batches {
            let batch = batch.map_err(|e| BundlebaseError::from(e.to_string()))?;
            if batch.num_rows() == 0 {
                continue;
            }
            let idx = batch.schema().index_of("id")?;
            let arr: StringArray = downcast_array(batch.column(idx).as_ref());
            if first.is_none() {
                first = Some(arr.value(0).to_string());
            }
            last = Some(arr.value(arr.len() - 1).to_string());
            total += batch.num_rows();
        }

        assert_eq!(20_000, total, "all rows must be scanned");
        assert_eq!(Some("0".to_string()), first, "row order must be preserved");
        assert_eq!(Some("19999".to_string()), last);
        assert!(
            n_batches >= 2,
            "full scan must yield multiple bounded batches (streaming), got {}",
            n_batches
        );
        Ok(())
    }

    /// A malformed line surfacing mid-stream must error the scan, not be
    /// silently skipped (row numbering integrity, bb-dvo.1).
    #[tokio::test]
    async fn test_full_scan_errors_on_malformed_line() -> Result<(), BundlebaseError> {
        let content = "{\"id\":\"0\"}\nnot json at all\n{\"id\":\"2\"}\n";
        write_jsonl("memory:///test_stream_malformed/data.jsonl", content).await?;

        let plugin = JsonlPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                "memory:///test_stream_malformed/data.jsonl",
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let ds = reader.data_source(None, &[], None, None).await?;
        let ctx = test_context().ctx;
        let stream = ds.open(0, ctx.task_ctx())?;
        let batches: Vec<_> = stream.collect::<Vec<_>>().await;
        assert!(
            batches.iter().any(|b| b.is_err()),
            "malformed line must produce a stream error"
        );
        Ok(())
    }

    /// Index-path reads must not drop rows longer than the 4KB read-ahead.
    #[tokio::test]
    async fn test_index_read_row_larger_than_read_ahead() -> Result<(), BundlebaseError> {
        let big = "x".repeat(8192);
        let content = format!(
            "{{\"id\":\"0\",\"val\":\"small\"}}\n{{\"id\":\"1\",\"val\":\"{}\"}}\n{{\"id\":\"2\",\"val\":\"last\"}}\n",
            big
        );
        write_jsonl("memory:///test_big_row/data.jsonl", &content).await?;

        let plugin = JsonlPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                "memory:///test_big_row/data.jsonl",
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let ids = read_rows_by_numbers(reader.as_ref(), &[0, 1, 2], "id").await?;
        assert_eq!(
            vec!["0", "1", "2"],
            ids,
            "rows longer than the read-ahead buffer must not be dropped"
        );

        let vals = read_rows_by_numbers(reader.as_ref(), &[1], "val").await?;
        assert_eq!(1, vals.len());
        assert_eq!(big, vals[0], "long row content must be read in full");

        Ok(())
    }

    /// Index-path reads must include the final line of a file with no trailing newline.
    #[tokio::test]
    async fn test_index_read_includes_final_line_without_trailing_newline(
    ) -> Result<(), BundlebaseError> {
        let content = "{\"id\":\"0\"}\n{\"id\":\"1\"}\n{\"id\":\"2\"}";
        write_jsonl("memory:///test_no_trailing_newline/data.jsonl", content).await?;

        let plugin = JsonlPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                "memory:///test_no_trailing_newline/data.jsonl",
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let ids = read_rows_by_numbers(reader.as_ref(), &[0, 1, 2], "id").await?;
        assert_eq!(
            vec!["0", "1", "2"],
            ids,
            "the final line without a trailing newline must not be dropped"
        );

        Ok(())
    }

    /// Logical row numbers skip blank lines (the parser produces no row for them),
    /// so byte-offset resolution must skip them too.
    #[tokio::test]
    async fn test_index_read_blank_lines_resolve_correct_rows() -> Result<(), BundlebaseError> {
        let content = "{\"id\":\"0\"}\n\n{\"id\":\"1\"}\n   \n{\"id\":\"2\"}\n";
        write_jsonl("memory:///test_blank_lines/data.jsonl", content).await?;

        let plugin = JsonlPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                "memory:///test_blank_lines/data.jsonl",
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let ids = read_rows_by_numbers(reader.as_ref(), &[2], "id").await?;
        assert_eq!(
            vec!["2"],
            ids,
            "logical row 2 must resolve to the third non-blank line"
        );

        Ok(())
    }

    /// The index (byte-offset) path must validate the file version like the
    /// full-scan path does, instead of silently reading the changed file.
    #[tokio::test]
    async fn test_index_read_validates_file_version() -> Result<(), BundlebaseError> {
        use bundlebase_io::file::IOReadWriteFile;
        use bundlebase_io::IOReadFile;

        let file = write_jsonl(
            "memory:///test_jsonl_version/data.jsonl",
            "{\"id\":\"0\"}\n{\"id\":\"1\"}\n",
        )
        .await?;
        let version = file.version().await?;

        // Pass a schema so the reader takes the cached-schema fast path,
        // mirroring query-time behavior.
        let schema: SchemaRef = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("id", arrow::datatypes::DataType::Utf8, true),
        ]));

        let plugin = JsonlPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                "memory:///test_jsonl_version/data.jsonl",
                &BlockId::generate(),
                &binding,
                Some(schema),
                None,
                Some(version),
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Modify the source file after the reader captured the expected version.
        file.write(bytes::Bytes::from_static(b"{\"id\":\"9\"}\n{\"id\":\"8\"}\n"))
            .await?;

        let alias = crate::ObjectIdAlias::from(1u16);
        let row_ids = vec![RowId::new(alias, 0)];
        let result = reader.data_source(None, &[], None, Some(&row_ids)).await;

        let err = match result {
            Ok(_) => panic!("index read on a changed file must error, not return rows"),
            Err(e) => e.to_string(),
        };
        assert!(
            err.contains("Version mismatch"),
            "expected version mismatch error, got: {}",
            err
        );

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
        let stats = reader
            .read_statistics()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected stats"))?;

        // Extract the row count from statistics
        let rows = stats
            .num_rows
            .get_value()
            .ok_or_else(|| BundlebaseError::from("Expected row count"))?;

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

    /// Statistics must count logical rows like the parser does — blank and
    /// whitespace-only lines produce no row, and a final line without a
    /// trailing newline still counts.
    #[tokio::test]
    async fn test_statistics_skips_blank_lines() -> Result<(), BundlebaseError> {
        let content = "{\"id\":\"0\"}\n\n{\"id\":\"1\"}\n   \n{\"id\":\"2\"}";
        write_jsonl("memory:///test_jsonl_stats_blank/data.jsonl", content).await?;

        let plugin = JsonlPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                "memory:///test_jsonl_stats_blank/data.jsonl",
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let stats = reader
            .read_statistics()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected stats"))?;
        let rows = stats
            .num_rows
            .get_value()
            .ok_or_else(|| BundlebaseError::from("Expected row count"))?;

        assert_eq!(
            &3, rows,
            "blank/whitespace-only lines must not count as rows"
        );

        Ok(())
    }
}
