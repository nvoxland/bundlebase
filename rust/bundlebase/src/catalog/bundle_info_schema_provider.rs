use crate::bundle::BundleCommit;
use crate::catalog;
use arrow::array::{Int32Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::{SchemaProvider, TableProvider};
use datafusion::datasource::MemTable;
use datafusion::error::DataFusionError;
use parking_lot::RwLock;
use std::sync::Arc;

/// SchemaProvider that exposes bundle metadata tables in the "bundle_info" schema.
/// Currently provides:
/// - `bundle_history`: Commit history for the bundle
#[derive(Debug)]
pub struct BundleInfoSchemaProvider {
    commits: Arc<RwLock<Vec<BundleCommit>>>,
}

impl BundleInfoSchemaProvider {
    pub fn new(commits: Arc<RwLock<Vec<BundleCommit>>>) -> Self {
        Self { commits }
    }
}

#[async_trait]
impl SchemaProvider for BundleInfoSchemaProvider {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn table_names(&self) -> Vec<String> {
        vec![catalog::BUNDLE_HISTORY_TABLE.to_string()]
    }

    async fn table(&self, name: &str) -> datafusion::error::Result<Option<Arc<dyn TableProvider>>> {
        if name == catalog::BUNDLE_HISTORY_TABLE {
            let commits = self.commits.read().clone();
            let table = BundleHistoryTable::new(commits)?;
            Ok(Some(Arc::new(table)))
        } else {
            Ok(None)
        }
    }

    fn table_exist(&self, name: &str) -> bool {
        name == catalog::BUNDLE_HISTORY_TABLE
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
