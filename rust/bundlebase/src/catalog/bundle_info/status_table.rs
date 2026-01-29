use crate::bundle::{BundleFacade, BundleStatus, CommandResponse};
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::sync::Arc;

/// TableProvider that queries bundle status (uncommitted changes) dynamically from the BundleFacade.
pub(super) struct BundleStatusTable {
    facade: Arc<dyn BundleFacade>,
}

impl std::fmt::Debug for BundleStatusTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleStatusTable").finish()
    }
}

impl BundleStatusTable {
    pub fn new(facade: Arc<dyn BundleFacade>) -> Self {
        Self { facade }
    }
}

#[async_trait]
impl TableProvider for BundleStatusTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        BundleStatus::schema()
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
        let status = self.facade.status();
        let batch = status
            .to_record_batch()
            .map_err(|e| datafusion::error::DataFusionError::External(e))?;
        let schema = BundleStatus::schema();
        let mem_table = MemTable::try_new(schema, vec![vec![batch]])?;
        mem_table.scan(state, projection, filters, limit).await
    }
}
