use bundlebase::bundle::BundleFacade;
use bundlebase::bundle_config::ConfigEntries;
use bundlebase_common::command_response::CommandResponse;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use futures::TryStreamExt;
use std::any::Any;
use std::sync::{Arc, Weak};

/// TableProvider that exposes bundle configuration entries from all layers.
pub(super) struct BundleConfigTable {
    facade: Weak<dyn BundleFacade>,
}

impl std::fmt::Debug for BundleConfigTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleConfigTable").finish()
    }
}

impl BundleConfigTable {
    pub fn new(facade: Weak<dyn BundleFacade>) -> Self {
        Self { facade }
    }

    fn facade(&self) -> Result<Arc<dyn BundleFacade>> {
        self.facade.upgrade().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal("Bundle has been dropped (while accessing bundle_info.config)".to_string())
        })
    }
}

#[async_trait]
impl TableProvider for BundleConfigTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        ConfigEntries::schema()
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
        let entries = ConfigEntries::from(
            self.facade()?.config().all_values()
                .map_err(|e| datafusion::error::DataFusionError::External(e))?
        );
        let stream = Box::new(entries)
            .into_stream()
            .map_err(|e| datafusion::error::DataFusionError::External(e))?;
        let batches: Vec<_> = stream.try_collect().await?;
        let mem_table = MemTable::try_new(self.schema(), vec![batches])?;
        mem_table.scan(state, projection, filters, limit).await
    }
}
