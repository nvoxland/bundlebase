use crate::bundle::command::BundleCommand;
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

/// TableProvider that lists all registered bundlebase SQL commands.
///
/// This table is static — it doesn't depend on bundle state.
/// Data comes from the `register_commands!` macro via `BundleCommand::command_metadata()`.
pub(super) struct CommandsTable {
    schema: SchemaRef,
}

impl std::fmt::Debug for CommandsTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandsTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl CommandsTable {
    pub fn new() -> Self {
        Self {
            schema: Self::table_schema(),
        }
    }

    fn table_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("syntax", DataType::Utf8, false),
            Field::new("mode", DataType::Utf8, false),
        ]))
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let metadata = BundleCommand::command_metadata();

        let names: Vec<&str> = metadata.iter().map(|(name, _, _)| *name).collect();
        let syntaxes: Vec<&str> = metadata.iter().map(|(_, syntax, _)| *syntax).collect();
        let modes: Vec<&str> = metadata.iter().map(|(_, _, mode)| *mode).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&self.schema),
            vec![
                Arc::new(StringArray::from(names)),
                Arc::new(StringArray::from(syntaxes)),
                Arc::new(StringArray::from(modes)),
            ],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl TableProvider for CommandsTable {
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
