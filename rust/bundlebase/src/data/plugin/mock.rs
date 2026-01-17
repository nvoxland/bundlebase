use crate::data::{DataReader, ObjectId};
use crate::BundlebaseError;
use arrow::array::RecordBatch;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use datafusion::common::{DataFusionError, Statistics};
use datafusion::datasource::source::DataSource;
use datafusion::execution::TaskContext;
use datafusion::logical_expr::Expr;
use datafusion::physical_expr::EquivalenceProperties;
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{DisplayFormatType, Partitioning, SendableRecordBatchStream};
use futures::stream;
use std::any::Any;
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;
use url::Url;

#[derive(Debug)]
pub struct MockReader {
    url: Url,
    schema: SchemaRef,
    block_id: ObjectId,
}

impl MockReader {
    pub fn with_schema(schema: SchemaRef) -> Self {
        let id = ObjectId::generate();
        Self {
            url: Url::parse(format!("mock://reader/{}", id).as_str()).unwrap(),
            schema,
            block_id: id,
        }
    }
}

#[async_trait]
impl DataReader for MockReader {
    fn url(&self) -> &Url {
        &self.url
    }

    fn block_id(&self) -> ObjectId {
        self.block_id
    }

    async fn read_schema(&self) -> Result<Option<SchemaRef>, BundlebaseError> {
        Ok(Some(self.schema.clone()))
    }

    async fn read_version(&self) -> Result<String, BundlebaseError> {
        Ok("MOCK VERSION".to_string())
    }

    async fn read_statistics(&self) -> Result<Option<Statistics>, BundlebaseError> {
        Ok(None)
    }

    async fn data_source(
        &self,
        _projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
        _row_ids: Option<&[crate::data::RowId]>,
    ) -> Result<Arc<dyn DataSource>, DataFusionError> {
        Ok(Arc::new(MockDataSource::new(self.schema.clone())))
    }
}

/// Mock DataSource that returns empty record batches
pub struct MockDataSource {
    schema: SchemaRef,
}

impl MockDataSource {
    pub fn new(schema: SchemaRef) -> Self {
        Self { schema }
    }
}

impl Debug for MockDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockDataSource")
            .field("schema", &self.schema)
            .finish()
    }
}

impl Display for MockDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "MockDataSource")
    }
}

impl DataSource for MockDataSource {
    fn open(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        // Return an empty stream with the correct schema
        let schema = self.schema.clone();
        let batches = vec![Ok(RecordBatch::new_empty(schema.clone()))];
        let stream = stream::iter(batches);
        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: DisplayFormatType, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "MockDataSource")
    }

    fn output_partitioning(&self) -> Partitioning {
        Partitioning::UnknownPartitioning(1)
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        EquivalenceProperties::new(self.schema.clone())
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
