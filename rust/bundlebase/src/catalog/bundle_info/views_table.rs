use crate::data::ObjectId;
use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use std::collections::HashMap;
use std::sync::Arc;

/// Helper struct for creating the bundle_views table
pub(super) struct BundleViewsTable;

impl BundleViewsTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
        ]))
    }

    pub fn new(views: HashMap<String, ObjectId>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        // Sort views by name for consistent ordering
        let mut view_list: Vec<_> = views.iter().collect();
        view_list.sort_by_key(|(name, _)| *name);

        let ids: Vec<String> = view_list.iter().map(|(_, id)| id.to_string()).collect();
        let names: Vec<&str> = view_list.iter().map(|(name, _)| name.as_str()).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(names)),
            ],
        )?;

        let batches = if views.is_empty() {
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}
