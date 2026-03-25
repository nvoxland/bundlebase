use bundlebase_data::DataContext;
use bundlebase_data::plugin::ReaderPlugin;
use bundlebase_data::{BlockId, DataReader};
use crate::Bundle;
use crate::BundleFacade;
use crate::BundlebaseError;
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::source::DataSource;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::Expr;
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::projection::ProjectionExec;
use datafusion::physical_plan::{DisplayFormatType, ExecutionPlan};
use std::any::Any;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;
use url::Url;

use bundlebase_data::RowId;

pub struct BundlebasePlugin;

#[async_trait]
impl ReaderPlugin for BundlebasePlugin {
    async fn reader(
        &self,
        source: &str,
        block_id: &BlockId,
        _bundle: &dyn DataContext,
        _schema: Option<SchemaRef>,
        _layout: Option<String>,
        _expected_version: Option<String>,
        _read_options: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<Option<Arc<dyn DataReader>>, BundlebaseError> {
        // Strip the scheme to get the target path/URL
        // Compound scheme: bundle+s3://bucket/key -> s3://bucket/key
        // Filesystem:      bundle:///path/to/dir   -> /path/to/dir
        // Also accepts "bundlebase" as an alias for "bundle"
        let target_path = if source.starts_with("bundle+") {
            &source["bundle+".len()..]
        } else if source.starts_with("bundlebase+") {
            &source["bundlebase+".len()..]
        } else if source.starts_with("bundle://") {
            &source["bundle://".len()..]
        } else if source.starts_with("bundlebase://") {
            &source["bundlebase://".len()..]
        } else {
            return Ok(None);
        };
        if target_path.is_empty() {
            return Err("No path specified in bundle:// URL".into());
        }

        // Open the target bundle
        // TODO: Forward credentials from the parent bundle's config once
        // DataContext exposes passed_config or a similar mechanism.
        let target_bundle = Bundle::open(target_path, None).await?;

        // Use the target bundle's URL as the reader URL
        let url = target_bundle.url();

        Ok(Some(Arc::new(BundlebaseDataReader {
            url,
            target_bundle,
            block_id: *block_id,
        })))
    }
}

struct BundlebaseDataReader {
    url: Url,
    target_bundle: Arc<Bundle>,
    block_id: BlockId,
}

impl Debug for BundlebaseDataReader {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundlebaseDataReader")
            .field("url", &self.url)
            .field("block_id", &self.block_id)
            .finish()
    }
}

#[async_trait]
impl DataReader for BundlebaseDataReader {
    fn url(&self) -> &Url {
        &self.url
    }

    fn block_id(&self) -> BlockId {
        self.block_id
    }

    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError> {
        let schema = self.target_bundle.schema().await?;
        Ok(Some(schema))
    }

    async fn read_statistics(&self) -> Result<Option<Statistics>, BundlebaseError> {
        use datafusion::common::stats::Precision;

        match self.target_bundle.num_rows().await {
            Ok(count) => {
                let stats = Statistics {
                    num_rows: Precision::Exact(count),
                    ..Default::default()
                };
                Ok(Some(stats))
            }
            Err(_) => Ok(None),
        }
    }

    async fn read_version(&self) -> Result<String, BundlebaseError> {
        Ok(self.target_bundle.version())
    }

    async fn data_source(
        &self,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
        _row_ids: Option<&[RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError> {
        // Get the target bundle's full query output as a DataFrame
        // Build the physical plan eagerly (in this async context)
        let df = (*self.target_bundle.dataframe().await.map_err(|e| {
            DataFusionError::Internal(format!("Failed to get bundlebase dataframe: {}", e))
        })?)
        .clone();

        let mut physical_plan: Arc<dyn ExecutionPlan> = df.create_physical_plan().await?;

        // Apply projection at the physical plan level using ProjectionExec
        if let Some(proj) = projection {
            let input_schema = physical_plan.schema();
            let mut exprs = Vec::with_capacity(proj.len());
            for &i in proj {
                let field = input_schema.field(i);
                let col_expr = datafusion::physical_expr::expressions::col(
                    field.name(),
                    &input_schema,
                )?;
                exprs.push((col_expr as Arc<dyn datafusion::physical_expr::PhysicalExpr>, field.name().clone()));
            }
            physical_plan = Arc::new(ProjectionExec::try_new(exprs, physical_plan)?);
        }

        let schema = physical_plan.schema();

        Ok(Arc::new(BundlebaseDataSource {
            physical_plan,
            schema,
        }))
    }
}

#[derive(Debug, Clone)]
struct BundlebaseDataSource {
    physical_plan: Arc<dyn ExecutionPlan>,
    schema: SchemaRef,
}

impl Display for BundlebaseDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bundlebase Data Source")
    }
}

impl DataSource for BundlebaseDataSource {
    fn open(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        self.physical_plan.execute(partition, context)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: DisplayFormatType, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "BundlebaseDataSource")
    }

    fn output_partitioning(&self) -> Partitioning {
        Partitioning::UnknownPartitioning(1)
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        EquivalenceProperties::new(Arc::clone(&self.schema))
    }

    fn partition_statistics(
        &self,
        _partition: Option<usize>,
    ) -> datafusion::common::Result<Statistics> {
        Ok(Statistics::new_unknown(&self.schema))
    }

    fn with_fetch(&self, _limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        None
    }

    fn fetch(&self) -> Option<usize> {
        None
    }

    fn try_swapping_with_projection(
        &self,
        _projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BundlebaseError;

    #[tokio::test]
    async fn test_wrong_scheme() -> Result<(), BundlebaseError> {
        let plugin = BundlebasePlugin;

        let binding = Bundle::empty(None).await?;
        let ctx: &dyn DataContext = &*binding;
        let result = plugin
            .reader(
                "file:///test.csv",
                &BlockId::generate(),
                ctx,
                None,
                None,
                None,
                None,
            )
            .await?;

        assert!(result.is_none());

        Ok(())
    }

    // test_open_and_read requires BundleBuilderExt convenience methods (attach/commit).
    // Covered by integration tests which can use the extension trait.
}
