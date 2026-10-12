//! Shared test helpers for DataSource filter wrappers.
//!
//! Provides a mock multi-partition `DataSource` and a stream collector so the
//! delete/update overlay filters can be unit-tested in isolation.

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::common::Statistics;
use datafusion::datasource::source::DataSource;
use datafusion::execution::{RecordBatchStream, SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::DisplayFormatType;
use futures::stream::{Stream, StreamExt};
use std::fmt::{self, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// A mock `DataSource` that reports a configurable partition count and emits
/// a fixed set of batches from `open(0, ..)`. Used to verify that filter
/// wrappers force single-partition execution and reject partition != 0.
#[derive(Debug)]
pub struct MockMultiPartitionSource {
    partitions: Vec<Vec<RecordBatch>>,
    reported_partitions: usize,
    schema: SchemaRef,
}

impl MockMultiPartitionSource {
    pub fn new(partitions: Vec<Vec<RecordBatch>>, reported_partitions: usize) -> Self {
        let schema = partitions
            .first()
            .and_then(|p| p.first())
            .map(|b| b.schema())
            .unwrap_or_else(|| Arc::new(arrow::datatypes::Schema::empty()));
        Self {
            partitions,
            reported_partitions,
            schema,
        }
    }
}

impl fmt::Display for MockMultiPartitionSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "MockMultiPartitionSource")
    }
}

impl DataSource for MockMultiPartitionSource {
    fn open(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        let batches = self.partitions.get(partition).cloned().unwrap_or_default();
        Ok(Box::pin(VecStream {
            batches: batches.into_iter(),
            schema: self.schema.clone(),
        }))
    }

    fn fmt_as(&self, _t: DisplayFormatType, f: &mut Formatter) -> fmt::Result {
        write!(f, "MockMultiPartitionSource")
    }

    fn output_partitioning(&self) -> Partitioning {
        Partitioning::UnknownPartitioning(self.reported_partitions)
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        EquivalenceProperties::new(self.schema.clone())
    }

    fn partition_statistics(
        &self,
        _partition: Option<usize>,
    ) -> datafusion::common::Result<Arc<Statistics>> {
        Ok(Arc::new(Statistics::new_unknown(&self.schema)))
    }

    fn apply_expressions(
        &self,
        _f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> datafusion::common::Result<TreeNodeRecursion>,
    ) -> datafusion::common::Result<TreeNodeRecursion> {
        // Test source holds no expressions.
        Ok(TreeNodeRecursion::Continue)
    }

    fn fetch(&self) -> Option<usize> {
        None
    }

    fn with_fetch(&self, _limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        None
    }

    fn try_swapping_with_projection(
        &self,
        _projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        Ok(None)
    }
}

struct VecStream {
    batches: std::vec::IntoIter<RecordBatch>,
    schema: SchemaRef,
}

impl Stream for VecStream {
    type Item = datafusion::common::Result<RecordBatch>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.batches.next().map(Ok))
    }
}

impl RecordBatchStream for VecStream {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}

/// Open partition 0 of a `DataSource` and collect all batches into a Vec.
pub async fn collect_source(source: Arc<dyn DataSource>) -> Vec<RecordBatch> {
    let ctx = Arc::new(TaskContext::default());
    let mut stream = source.open(0, ctx).expect("open");
    let mut out = Vec::new();
    while let Some(r) = stream.next().await {
        out.push(r.expect("batch"));
    }
    out
}
