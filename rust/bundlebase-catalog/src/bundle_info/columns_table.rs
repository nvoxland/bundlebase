use bundlebase::bundle::BundleFacade;
use arrow::array::{ArrayRef, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::sync::{Arc, Weak};

/// TableProvider that queries the bundle's columns dynamically from the BundleFacade.
pub(super) struct BundleColumnsTable {
    facade: Weak<dyn BundleFacade>,
}

impl std::fmt::Debug for BundleColumnsTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleColumnsTable").finish()
    }
}

impl BundleColumnsTable {
    pub fn new(facade: Weak<dyn BundleFacade>) -> Self {
        Self { facade }
    }

    fn facade(&self) -> Result<Arc<dyn BundleFacade>> {
        self.facade.upgrade().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal(
                "Bundle has been dropped (while accessing bundle_info.columns)".to_string(),
            )
        })
    }

    fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("column", DataType::Utf8, false),
            Field::new("type", DataType::Utf8, false),
            Field::new("nullable", DataType::Utf8, false),
            Field::new("source", DataType::Utf8, false),
        ]))
    }
}

#[async_trait]
impl TableProvider for BundleColumnsTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Self::output_schema()
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
        let facade = self.facade()?;
        let bundle_schema = facade.schema().await.map_err(|e| {
            datafusion::error::DataFusionError::External(e)
        })?;

        let columns: Vec<&str> = bundle_schema
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect();
        let types: Vec<String> = bundle_schema
            .fields()
            .iter()
            .map(|f| f.data_type().to_string())
            .collect();
        let nullables: Vec<&str> = bundle_schema
            .fields()
            .iter()
            .map(|f| if f.is_nullable() { "Yes" } else { "No" })
            .collect();

        // Compute column sources by mapping ColumnId → blocks → pack name
        let bs = facade.bundle_schema();
        let packs = facade.packs();
        let mut block_to_pack = std::collections::HashMap::new();
        for pack in packs.values() {
            for block in pack.blocks() {
                block_to_pack.insert(*block.id(), pack.name().to_string());
            }
        }
        let sources: Vec<String> = bundle_schema
            .fields()
            .iter()
            .map(|f| {
                bs.column_id(f.name())
                    .and_then(|col_id| {
                        bs.blocks_for_column(&col_id)
                            .first()
                            .and_then(|(block_id, _)| block_to_pack.get(block_id).cloned())
                    })
                    .unwrap_or_else(|| "computed".to_string())
            })
            .collect();

        let columns_array: ArrayRef = Arc::new(StringArray::from(columns));
        let types_array: ArrayRef = Arc::new(StringArray::from(types));
        let nullables_array: ArrayRef = Arc::new(StringArray::from(nullables));
        let sources_array: ArrayRef = Arc::new(StringArray::from(sources));

        let output_schema = Self::output_schema();
        let batch =
            RecordBatch::try_new(output_schema.clone(), vec![columns_array, types_array, nullables_array, sources_array])?;
        let mem_table = MemTable::try_new(output_schema, vec![vec![batch]])?;
        mem_table.scan(state, projection, filters, limit).await
    }
}
