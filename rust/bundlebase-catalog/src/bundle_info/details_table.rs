use bundlebase::bundle::BundleFacade;
use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::sync::{Arc, Weak};

/// TableProvider that queries bundle details dynamically from the BundleFacade.
pub(super) struct BundleDetailsTable {
    facade: Weak<dyn BundleFacade>,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundleDetailsTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleDetailsTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundleDetailsTable {
    pub fn new(facade: Weak<dyn BundleFacade>) -> Self {
        Self {
            facade,
            schema: Self::table_schema(),
        }
    }

    fn facade(&self) -> Result<Arc<dyn BundleFacade>> {
        self.facade.upgrade().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal("Bundle has been dropped (while accessing bundle_info.details)".to_string())
        })
    }

    fn table_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("description", DataType::Utf8, true),
            Field::new("url", DataType::Utf8, false),
            Field::new("from", DataType::Utf8, true),
            Field::new("version", DataType::Utf8, false),
        ]))
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let facade = self.facade()?;
        let id = facade.id();
        let name = facade.name();
        let description = facade.description();
        let url = facade.url().to_string();
        let from = facade.from().map(|u| u.to_string());
        let version = facade.version();

        let batch = RecordBatch::try_new(
            Arc::clone(&self.schema),
            vec![
                Arc::new(StringArray::from(vec![id.as_str()])),
                Arc::new(StringArray::from(vec![name.as_deref()])),
                Arc::new(StringArray::from(vec![description.as_deref()])),
                Arc::new(StringArray::from(vec![url.as_str()])),
                Arc::new(StringArray::from(vec![from.as_deref()])),
                Arc::new(StringArray::from(vec![version.as_str()])),
            ],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl TableProvider for BundleDetailsTable {
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
