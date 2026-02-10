use crate::bundle::{BundleFacade, DataBlock};
use crate::data::ObjectId;
use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::sync::Arc;

/// TableProvider that queries bundle blocks dynamically from the BundleFacade.
pub(super) struct BundleBlocksTable {
    facade: Arc<dyn BundleFacade>,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundleBlocksTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleBlocksTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundleBlocksTable {
    pub fn new(facade: Arc<dyn BundleFacade>) -> Self {
        Self {
            facade,
            schema: Self::table_schema(),
        }
    }

    fn table_schema() -> SchemaRef {
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

    fn build_batch(&self) -> Result<RecordBatch> {
        let packs = self.facade.packs();

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
            Arc::clone(&self.schema),
            vec![
                Arc::new(StringArray::from(
                    ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    versions.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    pack_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(pack_names)),
                Arc::new(StringArray::from(
                    source_ids.iter().map(|s| s.as_deref()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(source_locations)),
                Arc::new(StringArray::from(source_versions)),
            ],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl TableProvider for BundleBlocksTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let batch = self.build_batch()?;
        let mem_table = MemTable::try_new(self.schema.clone(), vec![vec![batch]])?;
        mem_table.scan(state, projection, filters, limit).await
    }
}
