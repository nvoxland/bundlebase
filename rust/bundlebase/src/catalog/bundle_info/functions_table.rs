use crate::bundle::BundleFacade;
use crate::bundle::function_definition::arrow_type_to_name;
use arrow::array::{BooleanArray, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::Session;
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::sync::{Arc, Weak};

/// TableProvider that exposes function entries in the `bundle_info.functions` table.
pub(super) struct BundleFunctionsTable {
    facade: Weak<dyn BundleFacade>,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundleFunctionsTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleFunctionsTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundleFunctionsTable {
    pub fn new(facade: Weak<dyn BundleFacade>) -> Self {
        Self {
            facade,
            schema: Self::table_schema(),
        }
    }

    fn facade(&self) -> Result<Arc<dyn BundleFacade>> {
        self.facade.upgrade().ok_or_else(|| {
            datafusion::error::DataFusionError::Internal("Bundle has been dropped".to_string())
        })
    }

    fn table_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new("input_types", DataType::Utf8, false),
            Field::new("return_type", DataType::Utf8, false),
            Field::new("runner", DataType::Utf8, false),
            Field::new("logic", DataType::Utf8, false),
            Field::new("platform", DataType::Utf8, false),
            Field::new("temporary", DataType::Boolean, false),
        ]))
    }

    fn build_batch(&self) -> Result<RecordBatch> {
        let mut entries = self.facade()?.function_entries();

        // Sort by name then input types for consistent ordering
        entries.sort_by(|a, b| {
            a.name.to_string().cmp(&b.name.to_string())
                .then_with(|| {
                    let a_types: Vec<String> = a.input_types.iter().map(|dt| arrow_type_to_name(dt)).collect();
                    let b_types: Vec<String> = b.input_types.iter().map(|dt| arrow_type_to_name(dt)).collect();
                    a_types.join(",").cmp(&b_types.join(","))
                })
        });

        let ids: Vec<String> = entries.iter().map(|e| e.id.to_string()).collect();
        let names: Vec<String> = entries.iter().map(|e| e.name.to_string()).collect();
        let kinds: Vec<String> = entries.iter().map(|e| e.kind.to_string()).collect();
        let input_types: Vec<String> = entries.iter().map(|e| {
            let types: Vec<String> = e.input_types.iter().map(|dt| arrow_type_to_name(dt)).collect();
            types.join(", ")
        }).collect();
        let return_types: Vec<String> = entries.iter().map(|e| arrow_type_to_name(&e.return_type)).collect();
        let runners: Vec<String> = entries.iter().map(|e| e.from.runtime_name().to_string()).collect();
        let logics: Vec<String> = entries.iter().map(|e| e.from.to_logic_string()).collect();
        let platforms: Vec<String> = entries.iter().map(|e| e.platform.to_string()).collect();
        let temporaries: Vec<bool> = entries.iter().map(|e| e.temporary).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&self.schema),
            vec![
                Arc::new(StringArray::from(ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(names.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(kinds.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(input_types.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(return_types.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(runners.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(logics.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(platforms.iter().map(|s| s.as_str()).collect::<Vec<_>>())),
                Arc::new(BooleanArray::from(temporaries)),
            ],
        )?;

        Ok(batch)
    }
}

#[async_trait]
impl TableProvider for BundleFunctionsTable {
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
