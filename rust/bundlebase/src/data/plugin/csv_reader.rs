use crate::bundle::BundleFacade;
use crate::data::plugin::file_reader::{FileFormatConfig, FilePlugin, FileReader};
use crate::data::plugin::ReaderPlugin;
use crate::data::{DataReader, LayoutRowIdProvider, LineOrientedFormat, ObjectId, RowId, RowIdProvider};
use crate::index::RowIdIndex;
use crate::io::plugin::object_store::ObjectStoreFile;
use crate::io::IOReadWriteDir;
use crate::BundlebaseError;
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::common::stats::Precision;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::file_format::csv::CsvFormat;
use datafusion::datasource::file_format::FileFormat;
use datafusion::datasource::physical_plan::{CsvSource, FileSource};
use datafusion::datasource::source::DataSource;
use datafusion::logical_expr::Expr;
use futures::stream::StreamExt;
use std::sync::Arc;
use url::Url;

/// Configuration for CSV format
#[derive(Debug, Clone, Default)]
pub struct CsvFormatConfig;

impl FileFormatConfig for CsvFormatConfig {
    fn extension(&self) -> &'static str {
        ".csv"
    }

    fn file_format(&self) -> Arc<dyn FileFormat> {
        Arc::new(CsvFormat::default())
    }

    fn file_source(&self, schema: SchemaRef) -> Arc<dyn FileSource> {
        Arc::new(CsvSource::new(schema))
    }

    fn line_oriented_format(&self) -> Option<LineOrientedFormat> {
        Some(LineOrientedFormat::Csv)
    }
}

/// CSV plugin - uses generic FilePlugin and creates CsvReader
#[derive(Default)]
pub struct CsvPlugin {
    inner: FilePlugin<CsvFormatConfig>,
}

#[async_trait]
impl ReaderPlugin for CsvPlugin {
    async fn reader(
        &self,
        source: &str,
        block_id: &ObjectId,
        bundle: &dyn BundleFacade,
        schema: Option<SchemaRef>,
        layout: Option<String>,
        expected_version: Option<String>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError> {
        if !self.inner.handles(source) {
            return Ok(None);
        }

        let reader = self
            .inner
            .reader(source, bundle, schema, expected_version)
            .await?;
        let layout = match layout {
            None => None,
            Some(x) => Some(ObjectStoreFile::from_str(
                x.as_str(),
                bundle.data_dir().as_ref(),
                bundle.config(),
            )?),
        };
        Ok(Some(Arc::new(CsvReader::new(reader, block_id, &layout))))
    }
}

pub struct CsvReader {
    inner: FileReader<CsvFormatConfig>,
    block_id: ObjectId,
    layout: Option<ObjectStoreFile>,
    rowid_provider: Option<Arc<dyn RowIdProvider>>,
}

impl CsvReader {
    pub fn new(
        inner: FileReader<CsvFormatConfig>,
        block_id: &ObjectId,
        layout: &Option<ObjectStoreFile>,
    ) -> Self {
        // Create provider if layout file exists
        let row_id_provider = layout.as_ref().map(|layout_file| {
            Arc::new(LayoutRowIdProvider::new(layout_file.clone())) as Arc<dyn RowIdProvider>
        });

        Self {
            inner,
            block_id: *block_id,
            layout: layout.clone(),
            rowid_provider: row_id_provider,
        }
    }
}

impl std::fmt::Debug for CsvReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CsvReader")
            .field("inner", &self.inner)
            .field("block_id", &self.block_id)
            .field("layout", &self.layout)
            .field("has_provider", &self.rowid_provider.is_some())
            .finish()
    }
}

#[async_trait]
impl DataReader for CsvReader {
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
        let (num_rows, file_bytes) = self.compute_statistics().await?;

        // Create statistics with actual row count and byte size
        let stats = Statistics {
            num_rows: Precision::Exact(num_rows),
            total_byte_size: Precision::Exact(file_bytes),
            ..Default::default()
        };

        Ok(Some(stats))
    }

    async fn build_layout(
        &self,
        data_dir: &dyn IOReadWriteDir,
    ) -> Result<Option<Box<dyn crate::io::IOReadFile>>, BundlebaseError> {
        let index_file = RowIdIndex::new()
            .build(
                self.inner.file().as_object_store_file(),
                data_dir,
                &self.block_id(),
                true,
            )
            .await?;

        Ok(Some(index_file))
    }

    fn rowid_provider(&self) -> Result<Arc<dyn RowIdProvider>, BundlebaseError> {
        Ok(self
            .rowid_provider
            .clone()
            .expect("CSV rowid_generator requires a layout file".into()))
    }
}

impl CsvReader {
    /// Count the number of CSV rows and get file size by reading the file
    /// Assumes standard CSV format with header row
    /// Returns (row_count, file_size_in_bytes)
    async fn compute_statistics(&self) -> Result<(usize, usize), BundlebaseError> {
        use object_store::GetOptions;

        // Get the object store and path
        let store = self.inner.url();
        let object_store = self.inner.object_store();
        let path = object_store::path::Path::parse(store.path())?;

        // Read the file
        let get_result = object_store.get_opts(&path, GetOptions::default()).await?;

        let mut reader = get_result.into_stream();

        let mut content = Vec::new();
        while let Some(chunk) = reader.next().await {
            let chunk = chunk.map_err(|e| Box::new(e) as BundlebaseError)?;
            content.extend_from_slice(&chunk);
        }

        // Get file size
        let file_size = content.len();

        // Count newlines to determine number of rows (including header)
        let mut row_count = content.iter().filter(|&&b| b == b'\n').count();

        // If the last line doesn't end with newline, add 1 for the last row
        if !content.is_empty() && content[content.len() - 1] != b'\n' {
            row_count += 1;
        }

        // Subtract 1 for the header row to get data rows
        let data_row_count = if row_count > 0 { row_count - 1 } else { 0 };

        Ok((data_row_count, file_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::plugin::ReaderPlugin;
    use crate::test_utils::test_datafile;
    use crate::Bundle;
    use arrow::array::{downcast_array, Array, StringArray};
    use futures::stream::StreamExt;

    #[tokio::test]
    async fn test_wrong_file_extension() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let binding = Bundle::empty().await?;
        let result = plugin
            .reader("file:///test.parquet", &1.into(), &*binding, None, None, None)
            .await?;

        assert!(result.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_csv_file() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let binding = Bundle::empty().await?;
        let invalid_reader = plugin
            .reader("file:///invalid.csv", &1.into(), &*binding, None, None, None)
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
        // Test complete CSV file read and data validation
        let plugin = CsvPlugin::default();

        let binding = Bundle::empty().await?;
        let reader = plugin
            .reader(
                test_datafile("customers-0-100.csv"),
                &1.into(),
                &*binding,
                None,
                None,
                None,
            )
            .await?
            .unwrap();

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
                &1.into(),
                &*binding,
                Some(schema),
                None,
                None,
            )
            .await?
            .unwrap();

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
    async fn test_schema() -> Result<(), BundlebaseError> {
        // Test that CSV schema correctly infers data types
        let plugin = CsvPlugin::default();
        let binding = Bundle::empty().await?;
        let reader = plugin
            .reader(
                test_datafile("customers-0-100.csv"),
                &1.into(),
                &*binding,
                None,
                None,
                None,
            )
            .await?
            .unwrap();

        let schema = reader.read_schema().await?.unwrap();

        // Build schema string with column names and types
        let schema_string = schema
            .fields()
            .iter()
            .map(|f| format!("{}: {}", f.name(), f.data_type()))
            .collect::<Vec<_>>()
            .join("\n");

        // Expected schema - CSV parser infers types: Index as Int64, Subscription Date as Date32, others as Utf8
        let expected_schema = "Index: Int64\n\
                               Customer Id: Utf8\n\
                               First Name: Utf8\n\
                               Last Name: Utf8\n\
                               Company: Utf8\n\
                               City: Utf8\n\
                               Country: Utf8\n\
                               Phone 1: Utf8\n\
                               Phone 2: Utf8\n\
                               Email: Utf8\n\
                               Subscription Date: Date32\n\
                               Website: Utf8";

        assert_eq!(schema_string, expected_schema);

        Ok(())
    }

    #[tokio::test]
    async fn test_statistics() -> Result<(), BundlebaseError> {
        let plugin = CsvPlugin::default();

        let binding = Bundle::empty().await?;
        let reader = plugin
            .reader(
                test_datafile("customers-0-100.csv"),
                &1.into(),
                &*binding,
                None,
                None,
                None,
            )
            .await?
            .unwrap();

        // Statistics should be available for a valid CSV file
        let stats = reader.read_statistics().await?;
        assert!(
            stats.is_some(),
            "Statistics should be available for CSV file"
        );

        let stats = stats.unwrap();

        // Extract actual row count from statistics
        let rows = stats.num_rows.get_value().unwrap();

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

    #[tokio::test]
    async fn test_extract_rowids_stream() -> Result<(), BundlebaseError> {
        use crate::BundleBuilder;

        let plugin = CsvPlugin::default();
        let block_id = ObjectId::from(1);

        // Create a bundle with a writable memory-based data directory
        let builder = BundleBuilder::create("memory:///test_csv_extract", None).await?;
        let binding = builder.bundle();
        let bundle_facade = binding.clone();

        // First, create a reader to build the layout
        let csv_url = test_datafile("customers-0-100.csv");
        let temp_reader = plugin
            .reader(csv_url, &block_id, &bundle_facade, None, None, None)
            .await?
            .unwrap();

        // Build the layout file
        let data_dir = binding.data_dir();
        let layout_file = temp_reader
            .build_layout(data_dir.as_ref())
            .await?
            .ok_or_else(|| BundlebaseError::from("Layout should be built for CSV"))?;

        // Read the schema
        let schema = temp_reader.read_schema().await?;

        // Now create a new reader with the layout and schema
        let reader = plugin
            .reader(
                csv_url,
                &block_id,
                &bundle_facade,
                schema,
                Some(layout_file.url().to_string()),
                None,
            )
            .await?
            .unwrap();

        // Extract rowids stream
        let mut stream = reader.extract_rowids_stream(binding.ctx(), None).await?;

        let mut total_rows = 0;
        let mut last_offset = 0u64;
        let block_id = reader.block_id();

        while let Some(result) = stream.next().await {
            let rowid_batch = result?;
            let num_rows = rowid_batch.batch.num_rows();
            total_rows += num_rows;

            // Verify that row_ids match the batch size
            assert_eq!(
                rowid_batch.row_ids.len(),
                num_rows,
                "Row IDs count should match batch row count"
            );

            // Verify each RowId has correct block_id and monotonically increasing offsets
            for (i, row_id) in rowid_batch.row_ids.iter().enumerate() {
                assert_eq!(
                    row_id.block_id(),
                    block_id,
                    "Row {} should have correct block_id",
                    i
                );

                // Verify offsets are monotonically increasing
                if i > 0 || last_offset > 0 {
                    assert!(
                        row_id.offset() >= last_offset,
                        "Row offset should be monotonically increasing"
                    );
                }

                last_offset = row_id.offset();

                // Verify size is encoded (size is a u8, always >= 0)
                let _size = row_id.size_mb();
            }
        }

        // Verify we got all 100 rows from the CSV
        assert_eq!(
            total_rows, 100,
            "Should have extracted row IDs for all 100 rows"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_extract_rowids_stream_with_projection() -> Result<(), BundlebaseError> {
        use crate::BundleBuilder;

        let plugin = CsvPlugin::default();
        let block_id = ObjectId::from(1);

        // Create a bundle with a writable memory-based data directory
        let builder = BundleBuilder::create("memory:///test_csv_extract_proj", None).await?;
        let binding = builder.bundle();
        let bundle_facade = binding.clone();

        // First, create a reader to build the layout
        let csv_url = test_datafile("customers-0-100.csv");
        let temp_reader = plugin
            .reader(csv_url, &block_id, &bundle_facade, None, None, None)
            .await?
            .unwrap();

        // Build the layout file
        let data_dir = binding.data_dir();
        let layout_file = temp_reader
            .build_layout(data_dir.as_ref())
            .await?
            .ok_or_else(|| BundlebaseError::from("Layout should be built for CSV"))?;

        // Read the schema
        let schema = temp_reader.read_schema().await?;

        // Now create a new reader with the layout and schema
        let reader = plugin
            .reader(
                csv_url,
                &block_id,
                &bundle_facade,
                schema,
                Some(layout_file.url().to_string()),
                None,
            )
            .await?
            .unwrap();

        // Get the full schema to determine projection indices
        let _schema = reader.read_schema().await?.unwrap();

        // Project only the first 3 columns
        let projection = vec![0, 1, 2];

        // Extract rowids stream with projection
        let mut stream = reader
            .extract_rowids_stream(binding.ctx(), Some(&projection))
            .await?;

        let mut total_rows = 0;

        while let Some(result) = stream.next().await {
            let rowid_batch = result?;
            let num_rows = rowid_batch.batch.num_rows();
            total_rows += num_rows;

            // Verify the batch has the projected columns
            assert_eq!(
                rowid_batch.batch.num_columns(),
                3,
                "Batch should have 3 projected columns"
            );

            // Verify row_ids match the batch size
            assert_eq!(
                rowid_batch.row_ids.len(),
                num_rows,
                "Row IDs count should match batch row count"
            );
        }

        // Verify we got all 100 rows
        assert_eq!(
            total_rows, 100,
            "Should have extracted row IDs for all 100 rows with projection"
        );

        Ok(())
    }
}
