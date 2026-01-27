use crate::bundle::BundleFacade;
use crate::data::plugin::file_reader::{FileFormatConfig, FilePlugin, FileReader};
use crate::data::plugin::ReaderPlugin;
use crate::data::{DataReader, ObjectId, RowId, RowIdBatch, SendableRowIdBatchStream};
use crate::BundlebaseError;
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::common::stats::Precision;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::file_format::FileFormat;
use datafusion::datasource::physical_plan::{FileSource, ParquetSource};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
use datafusion::parquet::arrow::async_reader::{
    ParquetObjectReader, ParquetRecordBatchStreamBuilder,
};
use datafusion::prelude::SessionContext;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use url::Url;

/// Configuration for Parquet format
#[derive(Debug, Clone, Default)]
pub struct ParquetFormatConfig;

impl FileFormatConfig for ParquetFormatConfig {
    fn extension(&self) -> &'static str {
        ".parquet"
    }

    fn file_format(&self) -> Arc<dyn FileFormat> {
        Arc::new(ParquetFormat::default())
    }

    fn file_source(&self, schema: SchemaRef) -> Arc<dyn FileSource> {
        Arc::new(ParquetSource::new(schema))
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
        block_id: &ObjectId,
        bundle: &dyn BundleFacade,
        schema: Option<SchemaRef>,
        _layout: Option<String>,
        expected_version: Option<String>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError> {
        if !self.inner.handles(source) {
            return Ok(None);
        }

        let reader = self
            .inner
            .reader(source, bundle, schema, expected_version)
            .await?;
        Ok(Some(Arc::new(ParquetDataReader::new(reader, *block_id))))
    }
}

#[derive(Debug)]
pub struct ParquetDataReader {
    inner: FileReader<ParquetFormatConfig>,
    block_id: ObjectId,
}

impl ParquetDataReader {
    pub fn new(inner: FileReader<ParquetFormatConfig>, block_id: ObjectId) -> Self {
        Self { inner, block_id }
    }
}

#[async_trait]
impl DataReader for ParquetDataReader {
    fn url(&self) -> &Url {
        self.inner.url()
    }

    fn block_id(&self) -> ObjectId {
        self.block_id
    }

    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError> {
        self.inner.read_schema().await
    }

    async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
        row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError> {
        self.inner
            .data_source(projection, filters, limit, row_ids)
            .await
    }

    async fn read_version(&self) -> Result<String, BundlebaseError> {
        self.inner.version().await
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
        _ctx: Arc<SessionContext>,
        _projection: Option<&Vec<usize>>,
    ) -> Result<SendableRowIdBatchStream, BundlebaseError> {
        // Get object store components
        let store = self.inner.file().store();
        let path = self.inner.file().store_path().clone();
        let block_id = self.block_id;

        // Create async Parquet reader
        let object_reader = ParquetObjectReader::new(store, path);
        let builder = ParquetRecordBatchStreamBuilder::new(object_reader)
            .await
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        let inner_stream = builder
            .build()
            .map_err(|e| Box::new(e) as BundlebaseError)?;

        // Transform stream to add RowId information using a wrapper struct
        // that implements Stream
        let block_id_bits = (block_id.as_u8() as u64) << 56;
        let wrapped = RowIdStreamWrapper {
            inner: Box::new(inner_stream),
            global_row_num: 0,
            block_id_bits,
        };

        Ok(Box::pin(wrapped))
    }
}

/// Wrapper that transforms a RecordBatch stream into a RowIdBatch stream
/// Adds sequential RowId information to each batch
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
    global_row_num: u64,
    block_id_bits: u64,
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

                // Generate RowIds for this batch
                for _ in 0..num_rows {
                    let row_id = self.block_id_bits | self.global_row_num;
                    row_ids.push(RowId::from(row_id));
                    self.global_row_num += 1;
                }

                Poll::Ready(Some(Ok(RowIdBatch::new(batch, row_ids))))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(Box::new(e) as BundlebaseError))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::test_datafile;
    use crate::{Bundle, BundlebaseError};
    use arrow::array::{downcast_array, StringViewArray};
    use futures::stream::StreamExt;

    #[tokio::test]
    async fn test_wrong_file_extension() -> Result<(), BundlebaseError> {
        // Parquet plugin should only adapt .parquet files
        let plugin = ParquetPlugin::default();

        let binding = Bundle::empty().await?;
        let result = plugin
            .reader("file:///test.csv", &1.into(), &*binding, None, None, None)
            .await?;

        assert!(result.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_parquet_file() -> Result<(), BundlebaseError> {
        let plugin = ParquetPlugin::default();

        let binding = Bundle::empty().await?;
        let invalid_reader = plugin
            .reader("file:///invalid.parquet", &1.into(), &*binding, None, None, None)
            .await?;

        assert!(invalid_reader.is_some());

        // Schema access should fail for nonexistent file
        let schema_result = invalid_reader.unwrap().read_schema().await;
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

        let binding = Bundle::empty().await?;
        let reader = plugin
            .reader(
                test_datafile("userdata.parquet"),
                &1.into(),
                &*binding,
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
                &1.into(),
                &*binding,
                Some(schema),
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let binding2 = Bundle::empty().await?;
        let ctx = binding2.ctx();
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
        let binding = Bundle::empty().await?;
        let reader = plugin
            .reader(
                test_datafile("userdata.parquet"),
                &1.into(),
                &*binding,
                None,
                None,
                None,
            )
            .await?
            .unwrap();

        let schema = reader.read_schema().await?.unwrap();

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

        let binding = Bundle::empty().await?;
        let reader = plugin
            .reader(
                test_datafile("userdata.parquet"),
                &1.into(),
                &*binding,
                None,
                None,
                None,
            )
            .await?
            .unwrap();

        // Statistics should be available for a valid Parquet file
        let stats = reader.read_statistics().await?;
        assert!(
            stats.is_some(),
            "Statistics should be available for Parquet file"
        );

        let stats = stats.unwrap();

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
}
