use crate::functions::function_batch_generator::FunctionBatchGenerator;
use crate::{BundlebaseError, DataGenerator};
use arrow::datatypes::SchemaRef;
use datafusion::common::{project_schema, Statistics};
use datafusion::datasource::source::{DataSource, DataSourceExec};
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::{EquivalenceProperties, LexOrdering, Partitioning};
use datafusion::physical_plan::memory::LazyMemoryExec;
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_plan::{DisplayFormatType, ExecutionPlan};
use std::any::Any;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct FunctionDataSource {
    /// The partitions to query
    generator: Arc<dyn DataGenerator>,
    /// Schema representing the data before projection
    schema: SchemaRef,
    /// Schema representing the data after the optional projection is applied
    projected_schema: SchemaRef,
    /// Optional projection
    projection: Option<Vec<usize>>,
    /// Sort information: one or more equivalent orderings
    sort_information: Vec<LexOrdering>,
    /// The maximum number of records to read from this plan. If `None`,
    /// all records after filtering are returned.
    fetch: Option<usize>,
}

impl FunctionDataSource {
    pub fn new(
        generator: Arc<dyn DataGenerator>,
        schema: SchemaRef,
        projection: Option<Vec<usize>>,
    ) -> Result<FunctionDataSource, BundlebaseError> {
        let projected_schema = project_schema(&schema, projection.as_ref())?;
        Ok(Self {
            generator,
            schema,
            projected_schema,
            projection,
            sort_information: vec![],
            fetch: None,
        })
    }

    pub fn try_new_exec(
        generator: Arc<dyn DataGenerator>,
        schema: SchemaRef,
        projection: Option<Vec<usize>>,
    ) -> datafusion::common::Result<Arc<DataSourceExec>> {
        let source = Self::new(generator, schema, projection)?;
        Ok(Arc::new(DataSourceExec::new(Arc::new(source))))
    }

    pub fn with_limit(mut self, limit: Option<usize>) -> Self {
        self.fetch = limit;
        self
    }

    pub fn projection(&self) -> &Option<Vec<usize>> {
        &self.projection
    }

    pub fn original_schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }
}

impl Display for FunctionDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Function Data Source")
    }
}

impl DataSource for FunctionDataSource {
    fn open(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        // Create LazyMemoryExec with the full schema (generators produce full batches)
        let memory_exec = LazyMemoryExec::try_new(
            self.schema.clone(),
            vec![Arc::new(parking_lot::RwLock::new(
                FunctionBatchGenerator::new(self.generator.clone()),
            ))],
        )?;

        // Apply projection if specified to match projected_schema
        let memory_exec = if let Some(ref projection) = self.projection {
            memory_exec.with_projection(Some(projection.clone()))
        } else {
            memory_exec
        };

        memory_exec.execute(partition, context)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: DisplayFormatType, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "FunctionDataSource")
    }

    fn output_partitioning(&self) -> Partitioning {
        Partitioning::UnknownPartitioning(1)
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        EquivalenceProperties::new_with_orderings(
            Arc::clone(&self.projected_schema),
            self.sort_information.clone(),
        )
    }

    fn partition_statistics(
        &self,
        _partition: Option<usize>,
    ) -> datafusion::common::Result<Statistics> {
        Ok(Statistics::new_unknown(&self.schema))
    }

    fn with_fetch(&self, limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        let source = self.clone();
        Some(Arc::new(source.with_limit(limit)))
    }

    fn fetch(&self) -> Option<usize> {
        self.fetch
    }

    fn try_swapping_with_projection(
        &self,
        _projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        // For now, we don't optimize projections at the source level
        Ok(None)
    }
}
