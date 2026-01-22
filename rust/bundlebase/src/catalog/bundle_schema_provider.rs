use crate::bundle::{BundleCommit, DataFrameHolder};
use crate::catalog;
use arrow::array::{Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::{SchemaProvider, Session, TableProvider};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{Expr, TableType};
use datafusion::physical_plan::ExecutionPlan;
use parking_lot::RwLock;
use std::sync::Arc;

/// SchemaProvider that exposes the bundle's cached dataframe as a "bundle" table
/// and the commit history as a "bundle_history" table
#[derive(Debug)]
pub struct BundleSchemaProvider {
    dataframe: DataFrameHolder,
    commits: Arc<RwLock<Vec<BundleCommit>>>,
}

impl BundleSchemaProvider {
    pub fn new(dataframe: DataFrameHolder, commits: Arc<RwLock<Vec<BundleCommit>>>) -> Self {
        Self { dataframe, commits }
    }
}

#[async_trait]
impl SchemaProvider for BundleSchemaProvider {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        vec![
            catalog::DATAFRAME_ALIAS.to_string(),
            catalog::BUNDLE_HISTORY_TABLE.to_string(),
        ]
    }

    async fn table(&self, name: &str) -> datafusion::error::Result<Option<Arc<dyn TableProvider>>> {
        if name == catalog::DATAFRAME_ALIAS {
            Ok(Some(Arc::new(CachedDataFrameTable::new(
                self.dataframe.clone(),
            ))))
        } else if name == catalog::BUNDLE_HISTORY_TABLE {
            let commits = self.commits.read().clone();
            let table = BundleHistoryTable::new(commits)?;
            Ok(Some(Arc::new(table)))
        } else {
            Ok(None)
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        name == catalog::DATAFRAME_ALIAS || name == catalog::BUNDLE_HISTORY_TABLE
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

/// Helper struct for creating the bundle_history table
struct BundleHistoryTable;

impl BundleHistoryTable {
    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("url", DataType::Utf8, true),
            Field::new("author", DataType::Utf8, false),
            Field::new("message", DataType::Utf8, false),
            Field::new("timestamp", DataType::Utf8, false),
            Field::new("change_count", DataType::Int32, false),
        ]))
    }

    fn new(commits: Vec<BundleCommit>) -> Result<MemTable, DataFusionError> {
        let schema = Self::schema();

        // Build arrays from commits
        let ids: Vec<i32> = (0..commits.len() as i32).collect();
        let urls: Vec<Option<String>> = commits
            .iter()
            .map(|c| c.url.as_ref().map(|u| u.to_string()))
            .collect();
        let authors: Vec<&str> = commits.iter().map(|c| c.author.as_str()).collect();
        let messages: Vec<&str> = commits.iter().map(|c| c.message.as_str()).collect();
        let timestamps: Vec<&str> = commits.iter().map(|c| c.timestamp.as_str()).collect();
        let change_counts: Vec<i32> = commits
            .iter()
            .map(|c| c.changes.len() as i32)
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(Int32Array::from(ids)),
                Arc::new(StringArray::from(urls)),
                Arc::new(StringArray::from(authors)),
                Arc::new(StringArray::from(messages)),
                Arc::new(StringArray::from(timestamps)),
                Arc::new(Int32Array::from(change_counts)),
            ],
        )?;

        let batches = if commits.is_empty() {
            vec![]
        } else {
            vec![vec![batch]]
        };

        MemTable::try_new(schema, batches)
    }
}
