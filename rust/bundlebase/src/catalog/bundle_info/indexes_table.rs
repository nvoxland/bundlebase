use crate::bundle::BundleFacade;
use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::sync::Arc;

/// TableProvider that queries bundle indexes dynamically from the BundleFacade.
pub(super) struct BundleIndexesTable {
    facade: Arc<dyn BundleFacade>,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundleIndexesTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleIndexesTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundleIndexesTable {
    pub fn new(facade: Arc<dyn BundleFacade>) -> Self {
        Self {
            facade,
            schema: Self::table_schema(),
        }
    }

    fn table_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("column", DataType::Utf8, false),
            Field::new("type", DataType::Utf8, false),
            Field::new("tokenizer", DataType::Utf8, true),
        ]))
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let indexes = self.facade.indexes();

        let ids: Vec<String> = indexes.iter().map(|idx| idx.id().to_string()).collect();
        let columns: Vec<&str> = indexes.iter().map(|idx| idx.column().as_str()).collect();
        let types: Vec<&str> = indexes
            .iter()
            .map(|idx| {
                if idx.is_text() {
                    "text"
                } else {
                    "column"
                }
            })
            .collect();
        let tokenizers: Vec<Option<String>> = indexes
            .iter()
            .map(|idx| {
                idx.index_type()
                    .tokenizer()
                    .map(|t| t.tantivy_tokenizer_name().to_string())
            })
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&self.schema),
            vec![
                Arc::new(StringArray::from(
                    ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(columns)),
                Arc::new(StringArray::from(types)),
                Arc::new(StringArray::from(
                    tokenizers
                        .iter()
                        .map(|t| t.as_deref())
                        .collect::<Vec<_>>(),
                )),
            ],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl TableProvider for BundleIndexesTable {
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
