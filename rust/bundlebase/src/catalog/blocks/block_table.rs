use crate::bundle::DataBlock;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::TableType;
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::sync::Arc;

/// TableProvider wrapper for DataBlock.
///
/// This provides a clean abstraction layer between the catalog's SchemaProvider
/// and the underlying DataBlock, delegating all TableProvider operations.
pub(super) struct BlockTable {
    block: Arc<DataBlock>,
}

impl BlockTable {
    pub fn new(block: Arc<DataBlock>) -> Self {
        Self { block }
    }
}

impl std::fmt::Debug for BlockTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockTable")
            .field("block_id", self.block.id())
            .finish()
    }
}

#[async_trait]
impl TableProvider for BlockTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.block.schema()
    }

    fn table_type(&self) -> TableType {
        self.block.table_type()
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> datafusion::common::Result<Vec<datafusion::logical_expr::TableProviderFilterPushDown>>
    {
        self.block.supports_filters_pushdown(filters)
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        self.block.scan(state, projection, filters, limit).await
    }
}
