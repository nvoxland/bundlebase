use crate::bundle::BundleStatus;
use arrow::array::{Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use std::sync::Arc;

/// Helper struct for creating the bundle_status table
pub(super) struct BundleStatusTable;

impl BundleStatusTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("change_id", DataType::Utf8, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("operation_count", DataType::Int32, false),
        ]))
    }

    pub fn new(status: BundleStatus) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();
        let changes = status.changes();

        // Build arrays from changes
        let ids: Vec<i32> = (0..changes.len() as i32).collect();
        let change_ids: Vec<String> = changes.iter().map(|c| c.id.to_string()).collect();
        let descriptions: Vec<&str> = changes.iter().map(|c| c.description.as_str()).collect();
        let operation_counts: Vec<i32> = changes
            .iter()
            .map(|c| c.operations.len() as i32)
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int32Array::from(ids)),
                Arc::new(StringArray::from(change_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(descriptions)),
                Arc::new(Int32Array::from(operation_counts)),
            ],
        )?;

        let batches = if changes.is_empty() {
            // Return empty batch with schema (one partition with zero rows)
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}
