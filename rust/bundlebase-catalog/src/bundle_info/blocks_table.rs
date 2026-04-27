use arrow::array::{ListBuilder, RecordBatch, StringArray, StringBuilder, UInt64Array};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use bundlebase::bundle::{BundleFacade, DataBlock};
use bundlebase_common::object_id::ObjectId;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::sync::{Arc, Weak};

/// TableProvider that queries bundle blocks dynamically from the BundleFacade.
pub(super) struct BundleBlocksTable {
    facade: Weak<dyn BundleFacade>,
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
    pub fn new(facade: Weak<dyn BundleFacade>) -> Self {
        Self {
            facade,
            schema: Self::table_schema(),
        }
    }

    fn facade(&self) -> Result<Arc<dyn BundleFacade>> {
        self.facade.upgrade().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal(
                "Bundle has been dropped (while accessing bundle_info.blocks)".to_string(),
            )
        })
    }

    fn table_schema() -> SchemaRef {
        // `source_locations` and `source_versions` are *parallel lists* — one
        // entry per `BatchedSource` on the block, indexed the same way. With
        // `MIN BATCH` a single bundle block can carry many source entries, so
        // we faithfully expose all of them rather than picking arbitrarily.
        // `source_count` is the list length, surfaced as its own column for
        // quick scanning / filtering without unnesting.
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("version", DataType::Utf8, false),
            Field::new("pack_id", DataType::Utf8, false),
            Field::new("pack_name", DataType::Utf8, false),
            Field::new("source_id", DataType::Utf8, true),
            Field::new("source_count", DataType::UInt64, false),
            Field::new(
                "source_locations",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
            Field::new(
                "source_versions",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                true,
            ),
        ]))
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let packs = self.facade()?.packs();

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

        // `source_count` plus parallel List<Utf8> columns for locations and
        // versions. Blocks with no `source_info` get `count = 0` and NULL
        // lists; blocks with a SourceInfo always emit a list (possibly empty).
        let mut source_counts: Vec<u64> = Vec::with_capacity(blocks.len());
        let mut location_builder: ListBuilder<StringBuilder> =
            ListBuilder::new(StringBuilder::new());
        let mut version_builder: ListBuilder<StringBuilder> =
            ListBuilder::new(StringBuilder::new());
        for (block, _, _) in &blocks {
            match block.source_info() {
                Some(info) => {
                    source_counts.push(info.batch_sources.len() as u64);
                    for bs in &info.batch_sources {
                        location_builder.values().append_value(&bs.location);
                        version_builder.values().append_value(&bs.version);
                    }
                    location_builder.append(true);
                    version_builder.append(true);
                }
                None => {
                    source_counts.push(0);
                    location_builder.append(false); // NULL list
                    version_builder.append(false);
                }
            }
        }

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
                Arc::new(UInt64Array::from(source_counts)),
                Arc::new(location_builder.finish()),
                Arc::new(version_builder.finish()),
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
