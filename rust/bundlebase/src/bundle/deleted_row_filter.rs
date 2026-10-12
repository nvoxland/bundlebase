//! Streaming filter that excludes deleted rows from a DataSource.
//!
//! Wraps an inner `DataSource` and drops rows whose ordinal position
//! (within the block) appears in the deleted row set.

use arrow::array::BooleanArray;
use arrow::compute::filter_record_batch;
use arrow::datatypes::SchemaRef;
use datafusion::common::Statistics;
use datafusion::datasource::source::DataSource;
use datafusion::execution::{RecordBatchStream, SendableRecordBatchStream, TaskContext};
use datafusion::physical_expr::projection::ProjectionExprs;
use datafusion::physical_expr::{EquivalenceProperties, Partitioning};
use datafusion::physical_plan::DisplayFormatType;
use futures::stream::Stream;
use std::any::Any;
use std::fmt::{self, Formatter};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// A DataSource wrapper that filters out rows at deleted ordinal positions.
#[derive(Debug, Clone)]
pub struct DeletedRowFilterDataSource {
    inner: Arc<dyn DataSource>,
    deleted_rows: Arc<Vec<u32>>,
}

impl DeletedRowFilterDataSource {
    pub fn new(inner: Arc<dyn DataSource>, deleted_rows: Arc<Vec<u32>>) -> Self {
        Self {
            inner,
            deleted_rows,
        }
    }
}

impl fmt::Display for DeletedRowFilterDataSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "DeletedRowFilter({} deleted)", self.deleted_rows.len())
    }
}

impl DataSource for DeletedRowFilterDataSource {
    fn open(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        // Deleted-row ordinals are a single global sequence numbered from 0,
        // so this filter is single-partition (see `output_partitioning`).
        // Reject any other partition rather than silently miscounting offsets.
        if partition != 0 {
            return Err(datafusion::common::DataFusionError::Internal(format!(
                "DeletedRowFilterDataSource is single-partition; got partition {}",
                partition
            )));
        }
        let inner_stream = self.inner.open(partition, context)?;
        let schema = inner_stream.schema();
        Ok(Box::pin(DeletedRowFilterStream {
            inner: inner_stream,
            deleted_rows: self.deleted_rows.clone(),
            row_offset: 0,
            schema,
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn fmt_as(&self, _t: DisplayFormatType, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "DeletedRowFilterDataSource({} deleted)",
            self.deleted_rows.len()
        )
    }

    fn output_partitioning(&self) -> Partitioning {
        // Force single-partition execution: ordinal row numbering is only
        // correct as one global sequence. If the inner source reports >1
        // partition, delegating would let each partition restart its
        // row_offset at 0 and delete the wrong ordinals.
        Partitioning::UnknownPartitioning(1)
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        self.inner.eq_properties()
    }

    fn partition_statistics(
        &self,
        partition: Option<usize>,
    ) -> datafusion::common::Result<Statistics> {
        // The inner statistics describe the RAW row set. Passing them through
        // unadjusted lets DataFusion's AggregateStatistics rule rewrite
        // COUNT(*) to the raw count — the deleted rows would be counted
        // without this filter ever running. Every deleted ordinal corresponds
        // to a row the inner source will emit, so an exact inner count stays
        // exact after subtraction. Column-level stats (min/max/null counts)
        // may no longer be tight once rows are removed — degrade to inexact.
        use datafusion::common::stats::Precision;
        let deleted = self.deleted_rows.len();
        let mut stats = self.inner.partition_statistics(partition)?;
        stats.num_rows = match stats.num_rows {
            Precision::Exact(n) => Precision::Exact(n.saturating_sub(deleted)),
            Precision::Inexact(n) => Precision::Inexact(n.saturating_sub(deleted)),
            Precision::Absent => Precision::Absent,
        };
        stats.total_byte_size = stats.total_byte_size.to_inexact();
        stats.column_statistics = stats
            .column_statistics
            .into_iter()
            .map(|c| c.to_inexact())
            .collect();
        Ok(stats)
    }

    fn fetch(&self) -> Option<usize> {
        // Don't advertise a fetch — the limit must be applied ABOVE this
        // filter (see `with_fetch`).
        None
    }

    fn with_fetch(&self, _limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        // Returning None means "I can't absorb the fetch", so DataFusion keeps
        // a separate LimitExec above this filter. Pushing the limit BELOW the
        // delete filter would let the inner source stop at `n` rows before
        // deletions are applied, yielding fewer than `n` surviving rows.
        None
    }

    fn try_swapping_with_projection(
        &self,
        _projection: &ProjectionExprs,
    ) -> datafusion::common::Result<Option<Arc<dyn DataSource>>> {
        Ok(None)
    }
}

/// Stream adapter that filters out deleted rows by ordinal position.
struct DeletedRowFilterStream {
    inner: SendableRecordBatchStream,
    deleted_rows: Arc<Vec<u32>>,
    row_offset: u32,
    schema: SchemaRef,
}

impl Stream for DeletedRowFilterStream {
    type Item = datafusion::common::Result<arrow::record_batch::RecordBatch>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(batch))) => {
                    let num_rows = batch.num_rows() as u32;
                    let offset = self.row_offset;
                    self.row_offset += num_rows;

                    // Binary search to find deleted rows in [offset, offset + num_rows)
                    let start = self.deleted_rows.partition_point(|&r| r < offset);
                    let end = self
                        .deleted_rows
                        .partition_point(|&r| r < offset + num_rows);

                    if start == end {
                        // No deleted rows in this batch — pass through
                        return Poll::Ready(Some(Ok(batch)));
                    }

                    let deleted_in_range = &self.deleted_rows[start..end];

                    if deleted_in_range.len() == num_rows as usize {
                        // All rows in this batch were deleted, get next batch
                        continue;
                    }

                    // Build boolean mask by flipping only deleted positions
                    let mut mask_buf = vec![true; num_rows as usize];
                    for &del_row in deleted_in_range {
                        mask_buf[(del_row - offset) as usize] = false;
                    }
                    let mask = BooleanArray::from(mask_buf);

                    match filter_record_batch(&batch, &mask) {
                        Ok(filtered) if filtered.num_rows() == 0 => continue,
                        Ok(filtered) => return Poll::Ready(Some(Ok(filtered))),
                        Err(e) => return Poll::Ready(Some(Err(e.into()))),
                    }
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl RecordBatchStream for DeletedRowFilterStream {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::test_support::{collect_source, MockMultiPartitionSource};
    use arrow::array::Int64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use datafusion::execution::TaskContext;

    fn batch(vals: &[i64]) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int64, false)]));
        RecordBatch::try_new(schema, vec![Arc::new(Int64Array::from(vals.to_vec()))]).unwrap()
    }

    fn vals(b: &RecordBatch) -> Vec<i64> {
        let arr = b.column(0).as_any().downcast_ref::<Int64Array>().unwrap();
        (0..arr.len()).map(|i| arr.value(i)).collect()
    }

    #[tokio::test]
    async fn test_forces_single_partition() {
        // Inner reports 3 partitions; the filter must report exactly 1.
        let inner = Arc::new(MockMultiPartitionSource::new(
            vec![vec![batch(&[0, 1, 2])]],
            3,
        ));
        let src = DeletedRowFilterDataSource::new(inner, Arc::new(vec![]));
        match src.output_partitioning() {
            Partitioning::UnknownPartitioning(n) => assert_eq!(n, 1),
            other => panic!("expected single partition, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_open_rejects_nonzero_partition() {
        let inner = Arc::new(MockMultiPartitionSource::new(
            vec![vec![batch(&[0, 1, 2])]],
            3,
        ));
        let src = DeletedRowFilterDataSource::new(inner, Arc::new(vec![]));
        let ctx = Arc::new(TaskContext::default());
        assert!(src.open(1, ctx).is_err());
    }

    #[tokio::test]
    async fn test_with_fetch_returns_none() {
        // The limit must NOT be pushed below the delete filter, so with_fetch
        // returns None and DataFusion keeps a LimitExec above us.
        let inner = Arc::new(MockMultiPartitionSource::new(
            vec![vec![batch(&[0, 1, 2])]],
            1,
        ));
        let src = DeletedRowFilterDataSource::new(inner, Arc::new(vec![1]));
        assert!(src.with_fetch(Some(2)).is_none());
    }

    #[tokio::test]
    async fn test_deletes_at_correct_ordinals_across_batches() {
        // Two batches: rows 0,1,2 then 3,4,5. Delete ordinals 1 and 4.
        let inner = Arc::new(MockMultiPartitionSource::new(
            vec![vec![batch(&[10, 11, 12]), batch(&[13, 14, 15])]],
            1,
        ));
        let src = Arc::new(DeletedRowFilterDataSource::new(inner, Arc::new(vec![1, 4])));
        let out = collect_source(src).await;
        let got: Vec<i64> = out.iter().flat_map(|b| vals(b)).collect();
        // ordinal 1 -> value 11, ordinal 4 -> value 14 removed
        assert_eq!(got, vec![10, 12, 13, 15]);
    }
}
