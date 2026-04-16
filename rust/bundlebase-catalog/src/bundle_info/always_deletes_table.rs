use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use bundlebase::bundle::BundleFacade;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::sync::{Arc, Weak};

/// TableProvider that exposes always-delete rules from the BundleFacade.
pub(super) struct BundleAlwaysDeletesTable {
    facade: Weak<dyn BundleFacade>,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundleAlwaysDeletesTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleAlwaysDeletesTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundleAlwaysDeletesTable {
    pub fn new(facade: Weak<dyn BundleFacade>) -> Self {
        Self {
            facade,
            schema: Self::table_schema(),
        }
    }

    fn facade(&self) -> Result<Arc<dyn BundleFacade>> {
        self.facade.upgrade().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal(
                "Bundle has been dropped (while accessing bundle_info.always_deletes)".to_string(),
            )
        })
    }

    fn table_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "where_clause",
            DataType::Utf8,
            false,
        )]))
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let facade = self.facade()?;
        let rules = facade.always_delete_rules();

        // Translate internal column names back to user-visible names for display
        let bs = facade.bundle_schema();
        let rules: Vec<String> = rules
            .iter()
            .map(|r| {
                let mut result = r.clone();
                for (id, name) in bs.columns() {
                    let internal = bundlebase::bundle::bundle_schema::generate_internal_name(id);
                    result = result.replace(&internal, name);
                }
                result
            })
            .collect();
        let clauses: Vec<&str> = rules.iter().map(|s| s.as_str()).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&self.schema),
            vec![Arc::new(StringArray::from(clauses))],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl TableProvider for BundleAlwaysDeletesTable {
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
