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

/// TableProvider that exposes always-update rules from the BundleFacade.
pub(super) struct BundleAlwaysUpdatesTable {
    facade: Weak<dyn BundleFacade>,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundleAlwaysUpdatesTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleAlwaysUpdatesTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundleAlwaysUpdatesTable {
    pub fn new(facade: Weak<dyn BundleFacade>) -> Self {
        Self {
            facade,
            schema: Self::table_schema(),
        }
    }

    fn facade(&self) -> Result<Arc<dyn BundleFacade>> {
        self.facade.upgrade().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal(
                "Bundle has been dropped (while accessing bundle_info.always_updates)".to_string(),
            )
        })
    }

    fn table_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("set_clause", DataType::Utf8, false),
            Field::new("where_clause", DataType::Utf8, false),
        ]))
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let facade = self.facade()?;
        let rules = facade.always_update_rules();

        // Translate internal column names back to user-visible names for display
        let bs = facade.bundle_schema();
        let translate = |s: &str| -> String {
            let mut result = s.to_string();
            for (id, name) in bs.columns() {
                let internal = bundlebase::bundle::bundle_schema::generate_internal_name(id);
                result = result.replace(&internal, name);
            }
            result
        };
        let set_clauses: Vec<String> = rules.iter().map(|r| translate(&r.set_clause)).collect();
        let where_clauses: Vec<String> = rules.iter().map(|r| translate(&r.where_clause)).collect();
        let set_refs: Vec<&str> = set_clauses.iter().map(|s| s.as_str()).collect();
        let where_refs: Vec<&str> = where_clauses.iter().map(|s| s.as_str()).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&self.schema),
            vec![
                Arc::new(StringArray::from(set_refs)),
                Arc::new(StringArray::from(where_refs)),
            ],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl TableProvider for BundleAlwaysUpdatesTable {
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
