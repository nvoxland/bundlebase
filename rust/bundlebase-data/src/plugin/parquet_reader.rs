use crate::plugin::file_reader::{FileFormatConfig, FilePlugin, FileReader};
use crate::plugin::ReaderPlugin;
use crate::DataContext;
use crate::{BlockId, DataReader, ObjectIdAlias, RowId, RowIdBatch, SendableRowIdBatchStream};
use arrow::datatypes::{DataType, SchemaRef};
use async_trait::async_trait;
use bundlebase_common::BundlebaseError;
use bundlebase_io::plugin::object_store::ObjectStoreFile;
use bundlebase_io::IOReadWriteDir;
use datafusion::common::stats::Precision;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::file_format::FileFormat;
use datafusion::datasource::physical_plan::{
    parquet::CachedParquetFileReaderFactory, FileSource, ParquetFileReaderFactory, ParquetSource,
};
use datafusion::datasource::source::DataSource;
use datafusion::execution::cache::cache_manager::FileMetadataCache;
use datafusion::logical_expr::Expr;
use datafusion::parquet::arrow::async_reader::{
    ParquetObjectReader, ParquetRecordBatchStreamBuilder,
};
use datafusion::prelude::SessionContext;
use futures::StreamExt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use url::Url;

/// Configuration for Parquet format
#[derive(Debug, Clone, Default)]
pub struct ParquetFormatConfig;

impl FileFormatConfig for ParquetFormatConfig {
    fn extensions(&self) -> &'static [&'static str] {
        &[".parquet"]
    }

    fn file_format(&self) -> Arc<dyn FileFormat> {
        Arc::new(ParquetFormat::default())
    }

    fn file_source(&self, schema: SchemaRef) -> Arc<dyn FileSource> {
        Arc::new(
            ParquetSource::new(schema)
                .with_pushdown_filters(true)
                .with_reorder_filters(true),
        )
    }
}

#[derive(Default)]
pub struct ParquetPlugin {
    inner: FilePlugin<ParquetFormatConfig>,
}

#[async_trait]
impl ReaderPlugin for ParquetPlugin {
    async fn reader(
        &self,
        source: &str,
        block_id: &BlockId,
        bundle: &dyn DataContext,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
        _read_options: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError> {
        let lower = source.to_lowercase();
        if !lower.ends_with(".parquet") {
            return Ok(None);
        }

        let reader = self
            .inner
            .reader(source, bundle, schema, expected_version)
            .await?;
        let metadata_cache = bundle
            .session_context()
            .runtime_env()
            .cache_manager
            .get_file_metadata_cache();
        let layout_file = match layout {
            None => None,
            Some(x) => Some(ObjectStoreFile::from_str(
                x.as_str(),
                bundle.data_context_dir().as_ref(),
                bundle.config_provider(),
            )?),
        };
        Ok(Some(Arc::new(ParquetDataReader::new(
            reader,
            *block_id,
            metadata_cache,
            layout_file,
        ))))
    }
}

#[derive(Debug)]
pub struct ParquetDataReader {
    inner: FileReader<ParquetFormatConfig>,
    block_id: BlockId,
    reader_factory: Arc<dyn ParquetFileReaderFactory>,
    /// Layout file written at attach time, containing rich column statistics.
    layout: Option<ObjectStoreFile>,
}

impl ParquetDataReader {
    pub fn new(
        inner: FileReader<ParquetFormatConfig>,
        block_id: BlockId,
        metadata_cache: Arc<dyn FileMetadataCache>,
        layout: Option<ObjectStoreFile>,
    ) -> Self {
        let store = inner.file().store();
        let reader_factory = Arc::new(CachedParquetFileReaderFactory::new(store, metadata_cache));
        Self {
            inner,
            block_id,
            reader_factory,
            layout,
        }
    }
}

impl ParquetDataReader {
    /// Extract basic min/max/null_count/distinct_count from the Parquet footer.
    /// Does not scan row data — only reads the file footer.
    async fn column_stats_from_footer(
        &self,
    ) -> Result<Vec<crate::page_map::ColumnStats>, BundlebaseError> {
        use crate::page_map::ColumnStats;

        // Read Parquet metadata (only fetches footer, no row data)
        let store = self.inner.file().store();
        let path = self.inner.file().store_path().clone();
        let object_reader = ParquetObjectReader::new(store, path);
        let builder = ParquetRecordBatchStreamBuilder::new(object_reader)
            .await
            .map_err(|e| {
                BundlebaseError::from(format!("Failed to read Parquet metadata: {}", e))
            })?;
        let metadata = builder.metadata().clone();

        let schema = match self.inner.schema() {
            Some(s) => s.clone(),
            None => return Ok(vec![]),
        };
        let num_cols = schema.fields().len();

        use crate::page_map::StatValue;
        use std::cmp::Ordering;

        let mut null_counts = vec![0u64; num_cols];
        let mut mins: Vec<Option<StatValue>> = vec![None; num_cols];
        let mut maxs: Vec<Option<StatValue>> = vec![None; num_cols];
        let mut distinct_counts = vec![0u64; num_cols];

        for row_group in metadata.row_groups() {
            for (col_idx, col_chunk) in row_group.columns().iter().enumerate() {
                if col_idx >= num_cols {
                    continue;
                }
                if let Some(stats) = col_chunk.statistics() {
                    if let Some(nc) = stats.null_count_opt() {
                        null_counts[col_idx] += nc;
                    }
                    let field = schema.field(col_idx);
                    let (mn, mx) = parquet_stats_to_stat_values(stats, field);
                    if let Some(m) = mn {
                        mins[col_idx] = Some(match &mins[col_idx] {
                            None => m,
                            Some(existing) => match m.cmp_to_stat(existing) {
                                Some(Ordering::Less) => m,
                                _ => existing.clone(),
                            },
                        });
                    }
                    if let Some(m) = mx {
                        maxs[col_idx] = Some(match &maxs[col_idx] {
                            None => m,
                            Some(existing) => match m.cmp_to_stat(existing) {
                                Some(Ordering::Greater) => m,
                                _ => existing.clone(),
                            },
                        });
                    }
                    if let Some(dc) = stats.distinct_count_opt() {
                        distinct_counts[col_idx] = distinct_counts[col_idx].max(dc);
                    }
                }
            }
        }

        let result = (0..num_cols)
            .map(|i| {
                let field = schema.field(i);
                let is_numeric = matches!(
                    field.data_type(),
                    DataType::Int8
                        | DataType::Int16
                        | DataType::Int32
                        | DataType::Int64
                        | DataType::UInt8
                        | DataType::UInt16
                        | DataType::UInt32
                        | DataType::UInt64
                        | DataType::Float16
                        | DataType::Float32
                        | DataType::Float64
                        | DataType::Decimal128(_, _)
                        | DataType::Decimal256(_, _)
                );
                ColumnStats {
                    null_count: null_counts[i],
                    min: mins[i].clone(),
                    max: maxs[i].clone(),
                    distinct_count: distinct_counts[i],
                    is_all_numeric: is_numeric,
                    ..Default::default()
                }
            })
            .collect();

        Ok(result)
    }
}

#[async_trait]
impl DataReader for ParquetDataReader {
    fn url(&self) -> &Url {
        self.inner.url()
    }
    fn block_id(&self) -> BlockId {
        self.block_id
    }
    fn format(&self) -> crate::attach_format::AttachFormat {
        crate::attach_format::AttachFormat::Parquet
    }
    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError> {
        self.inner.read_schema().await
    }
    async fn read_version(&self) -> Result<String, BundlebaseError> {
        self.inner.version().await
    }

    async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
        _row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError> {
        use datafusion::datasource::listing::PartitionedFile;
        use datafusion::datasource::physical_plan::FileScanConfigBuilder;

        let metadata = self
            .inner
            .file()
            .object_meta()
            .await
            .map_err(|e| {
                DataFusionError::Internal(format!("Failed to get object metadata: {}", e))
            })?
            .ok_or_else(|| {
                DataFusionError::Internal(format!(
                    "File metadata not available for: {}",
                    self.inner.file().url()
                ))
            })?;

        let partitioned_file = PartitionedFile::from(metadata);
        let schema = self.inner.schema().clone().expect("No schema set");
        let parquet_source = ParquetSource::new(schema)
            .with_pushdown_filters(true)
            .with_reorder_filters(true)
            .with_parquet_file_reader_factory(self.reader_factory.clone());
        let mut builder =
            FileScanConfigBuilder::new(self.inner.file().store_url(), Arc::new(parquet_source))
                .with_file(partitioned_file);
        if let Some(proj) = projection {
            builder = builder.with_projection_indices(Some(proj.to_vec()))?;
        }
        if let Some(lim) = limit {
            builder = builder.with_limit(Some(lim));
        }
        Ok(Arc::new(builder.build()))
    }

    async fn column_stats(&self) -> Result<Vec<crate::page_map::ColumnStats>, BundlebaseError> {
        use crate::page_map::PageMap;
        if let Some(ref layout_file) = self.layout {
            let layout = PageMap::load(layout_file).await?;
            if !layout.column_stats.is_empty() {
                return Ok(layout.column_stats);
            }
        }
        self.column_stats_from_footer().await
    }

    async fn build_layout(
        &self,
        data_dir: &dyn IOReadWriteDir,
    ) -> Result<Option<Box<dyn bundlebase_io::IOReadFile>>, BundlebaseError> {
        use crate::column_stats_builder::ColumnStatsBuilder;
        use crate::page_map::PageMap;
        use arrow::compute::cast;
        use arrow::record_batch::RecordBatch;
        use futures::stream;

        let schema = match self.inner.schema() {
            Some(s) => s.clone(),
            None => return Ok(None),
        };
        let num_cols = schema.fields().len();

        // Build an all-Utf8 schema so the ColumnStatsBuilder can process all columns uniformly.
        let utf8_schema = Arc::new(arrow::datatypes::Schema::new(
            schema
                .fields()
                .iter()
                .map(|f| Arc::new(f.as_ref().clone().with_data_type(DataType::Utf8)))
                .collect::<Vec<_>>(),
        ));

        let mut stats_builder = ColumnStatsBuilder::new(num_cols, &[]);
        let mut row_count = 0u64;

        // Stream all Parquet rows, casting to Utf8 for the stats builder.
        let store = self.inner.file().store();
        let path = self.inner.file().store_path().clone();
        let object_reader = ParquetObjectReader::new(store, path);
        let parquet_builder = ParquetRecordBatchStreamBuilder::new(object_reader)
            .await
            .map_err(|e| {
                BundlebaseError::from(format!("Failed to open Parquet for stats: {}", e))
            })?;
        let mut batch_stream = parquet_builder
            .build()
            .map_err(|e| BundlebaseError::from(format!("Failed to build Parquet stream: {}", e)))?;

        while let Some(batch_result) = batch_stream.next().await {
            let batch = batch_result
                .map_err(|e| BundlebaseError::from(format!("Parquet read error: {}", e)))?;
            row_count += batch.num_rows() as u64;

            // Cast each column to Utf8; skip columns where cast fails (complex types).
            let utf8_cols: Vec<Arc<dyn arrow::array::Array>> = (0..num_cols)
                .map(|i| -> Arc<dyn arrow::array::Array> {
                    let col = batch.column(i);
                    match cast(col.as_ref(), &DataType::Utf8) {
                        Ok(c) => c,
                        Err(_) => col.clone(),
                    }
                })
                .collect();

            if let Ok(utf8_batch) = RecordBatch::try_new(utf8_schema.clone(), utf8_cols) {
                stats_builder.process_batch(&utf8_batch);
            }
        }

        if row_count == 0 {
            return Ok(None);
        }

        // Parquet doesn't track raw file size here; pass 0 so the bloom
        // budget relies on the `BLOOM_BUDGET_FLOOR_BYTES` floor (5 MB).
        // Parquet has its own internal column stats at the row-group level,
        // so the bundlebase layout sidecar is rarely the hot path for
        // parquet queries anyway.
        let column_stats = stats_builder.finish(0);
        let layout = PageMap {
            total_rows: row_count,
            file_size: 0,
            pages: vec![],
            column_stats,
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

    async fn read_statistics(&self) -> Result<Option<Statistics>, BundlebaseError> {
        // Get object store components for stream-based reading
        let store = self.inner.file().store();
        let path = self.inner.file().store_path().clone();

        // Get file metadata (size, timestamps, etc.) without reading file content
        let object_meta = self
            .inner
            .file()
            .object_meta()
            .await?
            .ok_or_else(|| BundlebaseError::from("Parquet file not found"))?;
        let file_size = object_meta.size as usize;

        // Create async Parquet reader using ObjectStore (only reads metadata footer)
        let object_reader = ParquetObjectReader::new(store, path);
        let builder = ParquetRecordBatchStreamBuilder::new(object_reader)
            .await
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        // Extract row count from Parquet metadata (no data reading)
        let metadata = builder.metadata();
        let row_count = metadata.file_metadata().num_rows() as usize;

        // Create statistics with row count and file size
        let stats = Statistics {
            num_rows: Precision::Exact(row_count),
            total_byte_size: Precision::Exact(file_size),
            ..Default::default()
        };

        Ok(Some(stats))
    }

    async fn extract_rowids_stream(
        &self,
        block_ref: ObjectIdAlias,
        _ctx: Arc<SessionContext>,
        projection: Option<&Vec<usize>>,
    ) -> Result<SendableRowIdBatchStream, BundlebaseError> {
        // Get object store components
        let store = self.inner.file().store();
        let path = self.inner.file().store_path().clone();

        // Create async Parquet reader
        let object_reader = ParquetObjectReader::new(store, path);
        let mut builder = ParquetRecordBatchStreamBuilder::new(object_reader)
            .await
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        // Honor the caller's projection by translating the *Arrow* column
        // indices it gave us into a Parquet `ProjectionMask`. Without this
        // the indexer's `Some(vec![col_idx])` was silently discarded, so
        // `batch.column(0)` carried whatever happened to be the first
        // column in the file — not the column the index was supposed to
        // cover. That's how the claude-history bundle ended up with an
        // inverted index full of `agent_id` data labelled `search_text`.
        //
        // Caveat: `ProjectionMask::roots` indexes *root* (top-level)
        // columns. For the flat schemas we currently index this matches
        // Arrow column indices 1:1. If we ever start indexing nested
        // columns (struct/list children), root indices won't equal Arrow
        // leaf indices and this needs to switch to `ProjectionMask::leaves`
        // with a leaf-id translation step.
        if let Some(cols) = projection {
            let parquet_schema = builder.parquet_schema();
            let mask = datafusion::parquet::arrow::ProjectionMask::roots(
                parquet_schema,
                cols.iter().copied(),
            );
            builder = builder.with_projection(mask);
        }

        let inner_stream = builder
            .build()
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        // Transform stream to add RowId information using a wrapper struct
        // that implements Stream
        let wrapped = RowIdStreamWrapper {
            inner: Box::new(inner_stream),
            global_row_num: 0,
            block_ref,
        };

        Ok(Box::pin(wrapped))
    }
}

/// Wrapper that transforms a RecordBatch stream into a RowIdBatch stream.
/// Adds sequential logical RowId information to each batch.
struct RowIdStreamWrapper {
    inner: Box<
        dyn futures::stream::Stream<
                Item = Result<
                    arrow::record_batch::RecordBatch,
                    datafusion::parquet::errors::ParquetError,
                >,
            > + Unpin
            + Send,
    >,
    global_row_num: u32,
    block_ref: ObjectIdAlias,
}

impl futures::stream::Stream for RowIdStreamWrapper {
    type Item = Result<RowIdBatch, BundlebaseError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // Poll the inner stream
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(batch))) => {
                let num_rows = batch.num_rows();
                let mut row_ids = Vec::with_capacity(num_rows);

                // Generate logical RowIds for this batch (sequential row numbers)
                for _ in 0..num_rows {
                    row_ids.push(RowId::new(self.block_ref, self.global_row_num));
                    self.global_row_num = match self.global_row_num.checked_add(1) {
                        Some(n) => n,
                        None => {
                            return Poll::Ready(Some(Err(
                                "Row count exceeds u32::MAX (~4 billion rows)".into(),
                            )))
                        }
                    };
                }

                Poll::Ready(Some(RowIdBatch::new(batch, row_ids)))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(Box::new(e) as BundlebaseError))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Convert Parquet column chunk statistics to typed `StatValue` min/max pairs.
/// Returns (None, None) if statistics are not present.
fn parquet_stats_to_stat_values(
    stats: &datafusion::parquet::file::statistics::Statistics,
    field: &arrow::datatypes::Field,
) -> (
    Option<crate::page_map::StatValue>,
    Option<crate::page_map::StatValue>,
) {
    use crate::page_map::StatValue;
    use arrow::datatypes::{DataType, TimeUnit};
    use datafusion::parquet::file::statistics::Statistics as S;

    match stats {
        S::Boolean(v) => (
            v.min_opt().map(|b| StatValue::Boolean(*b)),
            v.max_opt().map(|b| StatValue::Boolean(*b)),
        ),
        S::Int32(v) => {
            // Map to the Arrow type this column actually has — Parquet Int32 can back
            // Arrow Date32, Time32, or plain Int32.
            let to_sv: Box<dyn Fn(i32) -> StatValue> = match field.data_type() {
                DataType::Date32 => Box::new(StatValue::Date32),
                DataType::Time32(TimeUnit::Second) => Box::new(StatValue::Time32Second),
                DataType::Time32(TimeUnit::Millisecond) => Box::new(StatValue::Time32Millisecond),
                DataType::Int8 => Box::new(|n| StatValue::Int8(n as i8)),
                DataType::Int16 => Box::new(|n| StatValue::Int16(n as i16)),
                _ => Box::new(StatValue::Int32),
            };
            (
                v.min_opt().map(|n| to_sv(*n)),
                v.max_opt().map(|n| to_sv(*n)),
            )
        }
        S::Int64(v) => {
            let to_sv: Box<dyn Fn(i64) -> StatValue> = match field.data_type() {
                DataType::Date64 => Box::new(StatValue::Date64),
                DataType::Time64(TimeUnit::Microsecond) => Box::new(StatValue::Time64Microsecond),
                DataType::Time64(TimeUnit::Nanosecond) => Box::new(StatValue::Time64Nanosecond),
                DataType::Timestamp(TimeUnit::Second, _) => Box::new(StatValue::TimestampSecond),
                DataType::Timestamp(TimeUnit::Millisecond, _) => {
                    Box::new(StatValue::TimestampMillisecond)
                }
                DataType::Timestamp(TimeUnit::Microsecond, _) => {
                    Box::new(StatValue::TimestampMicrosecond)
                }
                DataType::Timestamp(TimeUnit::Nanosecond, _) => {
                    Box::new(StatValue::TimestampNanosecond)
                }
                DataType::UInt8 => Box::new(|n| StatValue::UInt8(n as u8)),
                DataType::UInt16 => Box::new(|n| StatValue::UInt16(n as u16)),
                DataType::UInt32 => Box::new(|n| StatValue::UInt32(n as u32)),
                DataType::UInt64 => Box::new(|n| StatValue::UInt64(n as u64)),
                _ => Box::new(StatValue::Int64),
            };
            (
                v.min_opt().map(|n| to_sv(*n)),
                v.max_opt().map(|n| to_sv(*n)),
            )
        }
        S::Int96(_) => (None, None), // Int96 timestamps — skip, rarely used
        S::Float(v) => (
            v.min_opt().map(|f| StatValue::Float32(*f)),
            v.max_opt().map(|f| StatValue::Float32(*f)),
        ),
        S::Double(v) => (
            v.min_opt().map(|f| StatValue::Float64(*f)),
            v.max_opt().map(|f| StatValue::Float64(*f)),
        ),
        S::ByteArray(v) => (
            v.min_opt()
                .map(|b| StatValue::Utf8(String::from_utf8_lossy(b.data()).into_owned())),
            v.max_opt()
                .map(|b| StatValue::Utf8(String::from_utf8_lossy(b.data()).into_owned())),
        ),
        S::FixedLenByteArray(v) => (
            v.min_opt()
                .map(|b| StatValue::Utf8(String::from_utf8_lossy(b.data()).into_owned())),
            v.max_opt()
                .map(|b| StatValue::Utf8(String::from_utf8_lossy(b.data()).into_owned())),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{test_context, test_datafile};
    use arrow::array::{downcast_array, StringViewArray};
    use futures::stream::StreamExt;

    #[tokio::test]
    async fn test_wrong_file_extension() -> Result<(), BundlebaseError> {
        // Parquet plugin should only adapt .parquet files
        let plugin = ParquetPlugin::default();

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
            "ParquetPlugin should reject non-Parquet format"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_parquet_file() -> Result<(), BundlebaseError> {
        let plugin = ParquetPlugin::default();

        let binding = test_context();
        let invalid_reader = plugin
            .reader(
                "file:///invalid.parquet",
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
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
        // Test complete Parquet file read and data validation
        let plugin = ParquetPlugin::default();

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("userdata.parquet"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Expected column names from userdata.parquet
        let column_names = vec![
            "registration_dttm",
            "id",
            "first_name",
            "last_name",
            "email",
            "gender",
            "ip_address",
            "cc",
            "country",
            "birthdate",
            "salary",
            "title",
            "comments",
        ];

        // Validate schema
        let schema = reader
            .read_schema()
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected schema"))?;

        let actual_columns: Vec<_> = schema.fields().iter().map(|f| f.name().clone()).collect();

        assert_eq!(
            column_names, actual_columns,
            "Parquet schema should match expected columns"
        );

        // Validate data reading
        let reader = plugin
            .reader(
                test_datafile("userdata.parquet"),
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

        // Validate "first_name" column (index 2)
        let name_array: StringViewArray = downcast_array(row1.column(2).as_ref());
        assert_eq!("Amanda", name_array.value(0), "First name should be Amanda");
        assert_eq!(
            "Albert",
            name_array.value(1),
            "Second name should be Albert"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_schema() -> Result<(), BundlebaseError> {
        let plugin = ParquetPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("userdata.parquet"),
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

        // Build a comprehensive schema string representation
        let schema_string = schema
            .fields()
            .iter()
            .map(|f| format!("{}: {}", f.name(), f.data_type()))
            .collect::<Vec<_>>()
            .join("\n");

        // Expected schema with all column names and their data types
        let expected_schema = "registration_dttm: Timestamp(ns)\n\
                               id: Int32\n\
                               first_name: Utf8View\n\
                               last_name: Utf8View\n\
                               email: Utf8View\n\
                               gender: Utf8View\n\
                               ip_address: Utf8View\n\
                               cc: Utf8View\n\
                               country: Utf8View\n\
                               birthdate: Utf8View\n\
                               salary: Float64\n\
                               title: Utf8View\n\
                               comments: Utf8View";

        assert_eq!(schema_string, expected_schema);

        Ok(())
    }

    #[tokio::test]
    async fn test_statistics() -> Result<(), BundlebaseError> {
        let plugin = ParquetPlugin::default();

        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("userdata.parquet"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Statistics should be available for a valid Parquet file
        let stats = reader.read_statistics().await?;
        assert!(
            stats.is_some(),
            "Statistics should be available for Parquet file"
        );

        let stats = stats.expect("checked above");

        // Extract actual row count from statistics
        let rows = match stats.num_rows {
            Precision::Exact(n) | Precision::Inexact(n) => n,
            _ => 0,
        };

        // userdata.parquet has 1000 rows (extracted from Parquet metadata)
        assert_eq!(
            1000, rows,
            "Parquet statistics should return actual row count from metadata. Got {} rows",
            rows
        );

        // Extract the byte size from statistics
        let bytes = match stats.total_byte_size {
            Precision::Exact(n) | Precision::Inexact(n) => n,
            _ => 0,
        };

        // userdata.parquet is 113629 bytes
        assert_eq!(
            113629, bytes,
            "Parquet statistics should return correct file size in bytes. Got {} bytes",
            bytes
        );

        Ok(())
    }

    /// Regression for the FTS-empty-on-real-bundle bug: index_blocks.rs
    /// hands a 1-element projection to `extract_rowids_stream` (column index
    /// of the column being indexed) and then reads `batch.column(0)`,
    /// expecting that to be the projected column. Before the fix the
    /// projection was a leading-underscore parameter — silently discarded —
    /// so the indexer fed `batch.column(0)` (whatever the *first* column of
    /// the parquet was) into the inverted index instead of `search_text`.
    /// On the public claude-history bundle that means hundreds of thousands
    /// of `agent_id` strings ended up in the index labelled as `search_text`,
    /// so every FTS query returned 0 hits.
    #[tokio::test]
    async fn test_extract_rowids_stream_honors_projection() -> Result<(), BundlebaseError> {
        use crate::DataReader;
        let plugin = ParquetPlugin::default();
        let binding = test_context();
        let reader = plugin
            .reader(
                test_datafile("userdata.parquet"),
                &BlockId::generate(),
                &binding,
                None,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // userdata.parquet column order:
        //   0 registration_dttm, 1 id, 2 first_name, 3 last_name, 4 email,
        //   5 gender, 6 ip_address, 7 cc, 8 country, 9 birthdate, 10 salary,
        //   11 title, 12 comments
        // Project ONLY column 4 (email). After the fix, `batch.column(0)`
        // must be the email field — not registration_dttm.
        let block_ref = ObjectIdAlias::from(0u16);
        let ctx = test_context();
        let projection = vec![4usize];
        let mut stream = reader
            .extract_rowids_stream(block_ref, ctx.ctx.clone(), Some(&projection))
            .await?;

        let mut total_rows = 0;
        let mut first_col_name: Option<String> = None;
        while let Some(batch_result) = stream.next().await {
            let rib = batch_result?;
            let batch = &rib.batch;
            total_rows += batch.num_rows();
            // Capture the first projected column's name from the first batch.
            if first_col_name.is_none() && batch.num_columns() > 0 {
                first_col_name = Some(batch.schema().field(0).name().clone());
            }
            // Sanity: the projection must yield exactly one column.
            assert_eq!(
                batch.num_columns(),
                1,
                "projection [4] must yield a 1-column batch, got {} columns ({:?})",
                batch.num_columns(),
                batch.schema().fields().iter().map(|f| f.name()).collect::<Vec<_>>()
            );
        }

        assert_eq!(total_rows, 1000, "userdata.parquet has 1000 rows");
        assert_eq!(
            first_col_name.as_deref(),
            Some("email"),
            "extract_rowids_stream(projection=[4]) must surface the `email` column \
             at batch.column(0); silently dropping the projection puts \
             `registration_dttm` (column 0 of the file) there instead, which \
             is exactly how the inverted index ended up indexing the wrong \
             column data on the claude-history bundle."
        );
        Ok(())
    }
}
