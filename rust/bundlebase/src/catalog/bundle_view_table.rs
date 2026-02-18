use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::catalog::{Session, TableProvider};
use datafusion::dataframe::DataFrame;
use datafusion::datasource::TableType;
use datafusion::logical_expr::{Expr, LogicalPlan, LogicalPlanBuilder, TableProviderFilterPushDown};
use datafusion::physical_plan::ExecutionPlan;
use std::any::Any;
use std::borrow::Cow;
use std::sync::Arc;

/// A TableProvider that wraps a DataFusion DataFrame with filter pushdown support.
///
/// Like DataFusion's built-in `DataFrameTableProvider` (from `into_view()`), this exposes
/// the DataFrame's logical plan via `get_logical_plan()`, which enables the `InlineTableScan`
/// optimizer rule. This allows DataFusion to push filters through the plan tree directly
/// to `PackTable` -> `DataBlock`, where index-based query acceleration occurs.
///
/// Returns `Inexact` for filter pushdown (vs `Exact` in the built-in provider), ensuring
/// DataFusion also applies filters after the scan for correctness.
pub struct BundleViewTable {
    plan: LogicalPlan,
    schema: SchemaRef,
}

impl std::fmt::Debug for BundleViewTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleViewTable")
            .field("schema", &self.schema)
            .finish()
    }
}

impl BundleViewTable {
    pub fn new(df: DataFrame) -> Self {
        let schema = Arc::clone(df.schema().inner());
        let plan = df.into_unoptimized_plan();
        Self { plan, schema }
    }
}

#[async_trait]
impl TableProvider for BundleViewTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_logical_plan(&self) -> Option<Cow<'_, LogicalPlan>> {
        Some(Cow::Borrowed(&self.plan))
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
    ) -> datafusion::common::Result<Vec<TableProviderFilterPushDown>> {
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
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        // Build a new logical plan from the inner plan, applying filters/projection/limit.
        // This mirrors DataFusion's DataFrameTableProvider::scan() approach.
        let mut expr = LogicalPlanBuilder::from(self.plan.clone());

        // Apply filters - LogicalPlanBuilder::filter() handles column resolution
        let filter = filters.iter().cloned().reduce(|acc, new| acc.and(new));
        if let Some(filter) = filter {
            expr = expr.filter(filter)?;
        }

        // Apply projection using column indices (avoids column name resolution issues)
        if let Some(p) = projection {
            expr = expr.select(p.iter().copied())?;
        }

        // Apply limit
        if let Some(l) = limit {
            expr = expr.limit(0, Some(l))?;
        }

        let plan = expr.build()?;
        state.create_physical_plan(&plan).await
    }
}
