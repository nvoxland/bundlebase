use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use std::sync::Arc;

/// Helper struct for creating the bundle_details table
pub(super) struct BundleDetailsTable;

impl BundleDetailsTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("description", DataType::Utf8, true),
            Field::new("url", DataType::Utf8, false),
            Field::new("from", DataType::Utf8, true),
            Field::new("version", DataType::Utf8, false),
        ]))
    }

    pub fn new(
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        url: &str,
        from: Option<&str>,
        version: &str,
    ) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(vec![id])),
                Arc::new(StringArray::from(vec![name])),
                Arc::new(StringArray::from(vec![description])),
                Arc::new(StringArray::from(vec![url])),
                Arc::new(StringArray::from(vec![from])),
                Arc::new(StringArray::from(vec![version])),
            ],
        )?;

        MemTable::try_new(schema, vec![vec![batch]])
    }
}
