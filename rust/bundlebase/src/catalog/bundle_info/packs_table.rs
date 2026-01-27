use crate::bundle::BundleFacade;
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

/// TableProvider that queries bundle packs dynamically from the BundleFacade.
pub(super) struct BundlePacksTable {
    facade: Arc<dyn BundleFacade>,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundlePacksTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundlePacksTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundlePacksTable {
    pub fn new(facade: Arc<dyn BundleFacade>) -> Self {
        Self {
            facade,
            schema: Self::table_schema(),
        }
    }

    fn table_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("join_type", DataType::Utf8, true),
            Field::new("expression", DataType::Utf8, true),
        ]))
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let packs = self.facade.packs();

        // Sort packs by ID for consistent ordering
        let mut pack_list: Vec<_> = packs.values().collect();
        pack_list.sort_by_key(|p| *p.id());

        let ids: Vec<String> = pack_list.iter().map(|p| p.id().to_string()).collect();
        let names: Vec<&str> = pack_list.iter().map(|p| p.name()).collect();
        let join_types: Vec<Option<&str>> = pack_list
            .iter()
            .map(|p| p.join_type().map(|jt| jt.as_str()))
            .collect();
        let expressions: Vec<Option<&str>> = pack_list.iter().map(|p| p.expression()).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&self.schema),
            vec![
                Arc::new(StringArray::from(
                    ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(names)),
                Arc::new(StringArray::from(join_types)),
                Arc::new(StringArray::from(expressions)),
            ],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl TableProvider for BundlePacksTable {
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
