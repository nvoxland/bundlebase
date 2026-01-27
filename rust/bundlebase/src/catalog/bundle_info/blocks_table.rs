use crate::bundle::{DataBlock, Pack};
use crate::io::ObjectId;
use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use std::collections::HashMap;
use std::sync::Arc;

/// Helper struct for creating the bundle_blocks table
pub(super) struct BundleBlocksTable;

impl BundleBlocksTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("version", DataType::Utf8, false),
            Field::new("pack_id", DataType::Utf8, false),
            Field::new("pack_name", DataType::Utf8, false),
            Field::new("source_id", DataType::Utf8, true),
            Field::new("source_location", DataType::Utf8, true),
            Field::new("source_version", DataType::Utf8, true),
        ]))
    }

    pub fn new(packs: HashMap<ObjectId, Arc<Pack>>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        // Collect all blocks from all packs
        let mut blocks: Vec<(Arc<DataBlock>, ObjectId, String)> = Vec::new();
        for pack in packs.values() {
            let pack_id = *pack.id();
            let pack_name = pack.name().to_string();
            for block in pack.blocks() {
                blocks.push((block, pack_id, pack_name.clone()));
            }
        }

        // Sort blocks by ID for consistent ordering
        blocks.sort_by_key(|(b, _, _)| *b.id());

        let ids: Vec<String> = blocks.iter().map(|(b, _, _)| b.id().to_string()).collect();
        let versions: Vec<String> = blocks.iter().map(|(b, _, _)| b.version()).collect();
        let pack_ids: Vec<String> = blocks.iter().map(|(_, pid, _)| pid.to_string()).collect();
        let pack_names: Vec<&str> = blocks.iter().map(|(_, _, pn)| pn.as_str()).collect();
        let source_ids: Vec<Option<String>> = blocks
            .iter()
            .map(|(b, _, _)| b.source_info().map(|si| si.id.to_string()))
            .collect();
        let source_locations: Vec<Option<&str>> = blocks
            .iter()
            .map(|(b, _, _)| b.source_info().map(|si| si.location.as_str()))
            .collect();
        let source_versions: Vec<Option<&str>> = blocks
            .iter()
            .map(|(b, _, _)| b.source_info().map(|si| si.version.as_str()))
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(versions.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(pack_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(pack_names)),
                Arc::new(StringArray::from(
                    source_ids.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(source_locations)),
                Arc::new(StringArray::from(source_versions)),
            ],
        )?;

        let batches = if blocks.is_empty() {
            let empty_batch = RecordBatch::new_empty(Arc::clone(&schema));
            vec![vec![empty_batch]]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}
