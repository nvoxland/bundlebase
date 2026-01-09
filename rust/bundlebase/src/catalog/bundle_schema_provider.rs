use crate::bundle::DataFrameHolder;
use crate::catalog;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::{SchemaProvider, Session, TableProvider};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{Expr, TableType};
use datafusion::physical_plan::ExecutionPlan;
use std::sync::Arc;

/// SchemaProvider that exposes the bundle's cached dataframe as a "bundle" table
#[derive(Debug)]
pub struct BundleSchemaProvider {
    dataframe: DataFrameHolder,
}

impl BundleSchemaProvider {
    pub fn new(dataframe: DataFrameHolder) -> Self {
        Self { dataframe }
    }
}

#[async_trait]
impl SchemaProvider for BundleSchemaProvider {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        vec![catalog::DATAFRAME_ALIAS.to_string()]
    }

    async fn table(&self, name: &str) -> datafusion::error::Result<Option<Arc<dyn TableProvider>>> {
        if name == catalog::DATAFRAME_ALIAS {
            Ok(Some(Arc::new(CachedDataFrameTable::new(
                self.dataframe.clone(),
            ))))
        } else {
            Ok(None)
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        name == catalog::DATAFRAME_ALIAS
    }
}

/// TableProvider that returns execution plans from the cached DataFrame
#[derive(Debug)]
struct CachedDataFrameTable {
    dataframe: DataFrameHolder,
}

impl CachedDataFrameTable {
    fn new(dataframe: DataFrameHolder) -> Self {
        Self { dataframe }
    }
}

#[async_trait]
impl TableProvider for CachedDataFrameTable {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        let df = self.dataframe.dataframe();

        // Convert DFSchema to Arrow Schema
        SchemaRef::new(df.schema().as_arrow().clone())
    }

    fn table_type(&self) -> TableType {
        TableType::View
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>, DataFusionError> {
        // Get the cached dataframe
        let df = self.dataframe.dataframe();

        // Apply filters if any
        let mut df_filtered = df.as_ref().clone();
        for filter in filters {
            df_filtered = df_filtered.filter(filter.clone())?;
        }

        // Apply projection if specified
        if let Some(proj_indices) = projection {
            let schema = df_filtered.schema();
            let proj_exprs: Vec<Expr> = proj_indices
                .iter()
                .map(|&i| datafusion::logical_expr::col(schema.field(i).name()))
                .collect();
            df_filtered = df_filtered.select(proj_exprs)?;
        }

        // Apply limit if specified
        if let Some(n) = limit {
            df_filtered = df_filtered.limit(0, Some(n))?;
        }

        // Create the physical plan from the filtered/projected dataframe
        state.create_physical_plan(df_filtered.logical_plan()).await
    }
}
