use crate::bundle::Pack;
use crate::io::ObjectId;
use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use std::collections::HashMap;
use std::sync::Arc;

/// Helper struct for creating the bundle_packs table
pub(super) struct BundlePacksTable;

impl BundlePacksTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("join_type", DataType::Utf8, true),
            Field::new("expression", DataType::Utf8, true),
        ]))
    }

    pub fn new(packs: HashMap<ObjectId, Arc<Pack>>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        // Sort packs by ID for consistent ordering
        let mut pack_list: Vec<_> = packs.values().collect();
        pack_list.sort_by_key(|p| *p.id());

        let ids: Vec<String> = pack_list.iter().map(|p| p.id().to_string()).collect();
        let names: Vec<&str> = pack_list.iter().map(|p| p.name()).collect();
        let join_types: Vec<Option<&str>> = pack_list
            .iter()
            .map(|p| p.join_type().map(|jt| jt.as_str()))
            .collect();
        let expressions: Vec<Option<&str>> = pack_list
            .iter()
            .map(|p| p.expression())
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(names)),
                Arc::new(StringArray::from(join_types)),
                Arc::new(StringArray::from(expressions)),
            ],
        )?;

        let batches = if packs.is_empty() {
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}
