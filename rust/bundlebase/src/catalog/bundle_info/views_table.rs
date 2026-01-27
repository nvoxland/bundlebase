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

/// TableProvider that queries bundle views dynamically from the BundleFacade.
pub(super) struct BundleViewsTable {
    facade: Arc<dyn BundleFacade>,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundleViewsTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleViewsTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundleViewsTable {
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
        ]))
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let views = self.facade.views_by_name();

        // Sort views by name for consistent ordering
        let mut view_list: Vec<_> = views.iter().collect();
        view_list.sort_by_key(|(name, _)| *name);

        let ids: Vec<String> = view_list.iter().map(|(_, id)| id.to_string()).collect();
        let names: Vec<&str> = view_list.iter().map(|(name, _)| name.as_str()).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&self.schema),
            vec![
                Arc::new(StringArray::from(
                    ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(names)),
            ],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl TableProvider for BundleViewsTable {
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
