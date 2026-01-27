use crate::index::IndexDefinition;
use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use std::sync::Arc;

/// Helper struct for creating the bundle_indexes table
pub(super) struct BundleIndexesTable;

impl BundleIndexesTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("column", DataType::Utf8, false),
            Field::new("type", DataType::Utf8, false),
            Field::new("tokenizer", DataType::Utf8, true),
        ]))
    }

    pub fn new(indexes: Vec<Arc<IndexDefinition>>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        let ids: Vec<String> = indexes.iter().map(|idx| idx.id().to_string()).collect();
        let columns: Vec<&str> = indexes.iter().map(|idx| idx.column().as_str()).collect();
        let types: Vec<&str> = indexes
            .iter()
            .map(|idx| {
                if idx.is_text() {
                    "text"
                } else {
                    "column"
                }
            })
            .collect();
        let tokenizers: Vec<Option<String>> = indexes
            .iter()
            .map(|idx| {
                idx.index_type()
                    .tokenizer()
                    .map(|t| t.tantivy_tokenizer_name().to_string())
            })
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(columns)),
                Arc::new(StringArray::from(types)),
                Arc::new(StringArray::from(
                    tokenizers
                        .iter()
                        .map(|t| t.as_deref())
                        .collect::<Vec<_>>(),
                )),
            ],
        )?;

        let batches = if indexes.is_empty() {
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}
