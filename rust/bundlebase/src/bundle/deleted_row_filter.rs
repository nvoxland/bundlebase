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
        self.inner.output_partitioning()
    }

    fn eq_properties(&self) -> EquivalenceProperties {
        self.inner.eq_properties()
    }

    fn partition_statistics(
        &self,
        partition: Option<usize>,
    ) -> datafusion::common::Result<Statistics> {
        self.inner.partition_statistics(partition)
    }

    fn fetch(&self) -> Option<usize> {
        self.inner.fetch()
    }

    fn with_fetch(&self, limit: Option<usize>) -> Option<Arc<dyn DataSource>> {
        self.inner.with_fetch(limit).map(|inner| {
            Arc::new(DeletedRowFilterDataSource {
                inner,
                deleted_rows: self.deleted_rows.clone(),
            }) as Arc<dyn DataSource>
        })
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
