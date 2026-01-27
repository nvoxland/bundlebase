use crate::bundle::BundleFacade;
use crate::data::plugin::file_reader::{FileFormatConfig, FilePlugin, FileReader};
use crate::data::plugin::ReaderPlugin;
use crate::data::{DataReader, LineOrientedFormat, ObjectId};
use crate::index::RowIdIndex;
use crate::io::IOReadWriteDir;
use crate::BundlebaseError;
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
pub struct JsonFormatConfig;

impl FileFormatConfig for JsonFormatConfig {
    fn extension(&self) -> &'static str {
        ".json"
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

/// JSON plugin - uses generic FilePlugin and creates JsonReader
#[derive(Default)]
pub struct JsonPlugin {
    inner: FilePlugin<JsonFormatConfig>,
}

#[async_trait]
impl ReaderPlugin for JsonPlugin {
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
        Ok(Some(Arc::new(JsonReader::new(reader, *block_id))))
    }
}

#[derive(Debug)]
pub struct JsonReader {
    inner: FileReader<JsonFormatConfig>,
    block_id: ObjectId,
}

impl JsonReader {
    pub fn new(inner: FileReader<JsonFormatConfig>, block_id: ObjectId) -> Self {
        Self { inner, block_id }
    }
}

#[async_trait]
impl DataReader for JsonReader {
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
        row_ids: Option<&[crate::data::RowId]>,
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
                false,
            )
            .await?;

        Ok(Some(index_file))
    }
}

impl JsonReader {
    /// Count the number of JSON rows and get file size by reading the file
    /// Assumes line-delimited JSON format (JSONL)
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

        // Count newlines (each line is a JSON object in JSONL format)
        let row_count = content.iter().filter(|&&b| b == b'\n').count();

        // If the last line doesn't end with newline, add 1 for the last object
        let row_count = if !content.is_empty() && content[content.len() - 1] != b'\n' {
            row_count + 1
        } else {
            row_count
        };

        Ok((row_count, file_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::plugin::ReaderPlugin;
    use crate::test_utils::test_datafile;
    use crate::Bundle;
    use arrow::array::{downcast_array, Array, StringArray};
    use datafusion::common::stats::Precision;
    use futures::stream::StreamExt;

    #[tokio::test]
    async fn test_wrong_file_extension() -> Result<(), BundlebaseError> {
        // JSON plugin should only adapt .json files
        let plugin = JsonPlugin::default();

        let binding = Bundle::empty().await?;
        let result = plugin
            .reader("file:///test.csv", &1.into(), &*binding, None, None, None)
            .await?;

        assert!(result.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_json_file() -> Result<(), BundlebaseError> {
        let plugin = JsonPlugin::default();

        let binding = Bundle::empty().await?;
        let invalid_reader = plugin
            .reader("file:///invalid.json", &1.into(), &*binding, None, None, None)
            .await?;

        assert!(
            invalid_reader.is_some(),
            "Plugin should return reader for .json URL even if file doesn't exist"
        );

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
        // Test complete JSON file read and data validation
        let plugin = JsonPlugin::default();

        let binding = Bundle::empty().await?;
        let reader = plugin
            .reader(
                test_datafile("objects.json"),
                &1.into(),
                &*binding,
                None,
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        // Expected column names from objects.json
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
                test_datafile("objects.json"),
                &1.into(),
                &*binding,
                Some(schema),
                None,
                None,
            )
            .await?
            .ok_or_else(|| BundlebaseError::from("Expected reader"))?;

        let binding2 = Bundle::empty().await?;
        let ctx = &binding2.ctx();
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
        let plugin = JsonPlugin::default();

        let binding = Bundle::empty().await?;
        let reader = plugin
            .reader(
                test_datafile("objects.json"),
                &1.into(),
                &*binding,
                None,
                None,
                None,
            )
            .await?
            .unwrap();

        // Statistics should be available for a valid JSON file
        let stats = reader.read_statistics().await?.unwrap();

        // Extract the row count from statistics
        let rows = stats.num_rows.get_value().unwrap();

        // Now JSON statistics should return the actual row count by reading the file
        // objects.json has 4 JSON objects (4 lines in JSONL format)
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

        // objects.json is 280 bytes
        assert_eq!(
            280, bytes,
            "JSON statistics should return correct file size in bytes. Got {} bytes",
            bytes
        );

        Ok(())
    }
}
