use crate::bundle::Pack;
use crate::io::ObjectId;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::datasource::TableType;
use datafusion::error::Result;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::{union::UnionExec, ExecutionPlan};
use std::any::Any;
use std::sync::Arc;

/// Custom TableProvider that represents a UNION of all blocks in a pack.
///
/// This table lazily constructs the UNION when scanned, maintaining the streaming
/// execution model. Multiple blocks in a pack are combined using UNION BY NAME.
pub struct PackTable {
    pack_id: ObjectId,
    pack: Arc<Pack>,
    schema: SchemaRef,
}

impl std::fmt::Debug for PackTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackUnionTable")
            .field("pack_id", &self.pack_id)
            .field("pack", &self.pack)
            .field("schema", &self.schema)
            .finish()
    }
}

impl PackTable {
    pub fn new(pack_id: ObjectId, pack: Arc<Pack>) -> Result<Self> {
        // Get schema from first block
        let blocks = pack.blocks();

        let schema = blocks.first().ok_or_else(|| {
            datafusion::error::DataFusionError::Plan(format!(
                "Pack {} has no blocks",
                pack_id
            ))
        })?.schema();

        Ok(Self {
            pack_id,
            pack,
            schema,
        })
    }
}

#[async_trait]
impl TableProvider for PackTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::View
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> datafusion::common::Result<Vec<datafusion::logical_expr::TableProviderFilterPushDown>>
    {
        use datafusion::logical_expr::TableProviderFilterPushDown;

        // Return Inexact for all filters - this tells DataFusion:
        // - "Pass the filters to scan() so we can use them for index optimization"
        // - "But still apply them afterwards to ensure correctness"
        //
        // This enables index-based query optimization while maintaining correctness.
        Ok(filters
            .iter()
            .map(|_| TableProviderFilterPushDown::Inexact)
            .collect())
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        log::debug!(
            "PackUnionTable.scan() called with projection: {:?}, filters: {:?}",
            projection,
            filters
        );
        let blocks = self.pack.blocks();

        // Scan each block to get its physical plan
        let mut inputs: Vec<Arc<dyn ExecutionPlan>> = Vec::new();
        for block in &blocks {
            let plan = block.scan(state, projection, filters, limit).await?;
            inputs.push(plan);
        }

        // If only one block, return its plan directly
        if let [plan] = inputs.as_slice() {
            return Ok(plan.clone());
        }

        // Create a UnionExec to combine all block plans
        Ok(UnionExec::try_new(inputs)?)
    }
}

// Unit tests are covered by integration tests in the main test suite
