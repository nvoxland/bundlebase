use crate::data::rowid_provider::RowIdProvider;
use crate::data::{RowId, RowIdBatch};
use crate::BundlebaseError;
use arrow::record_batch::RecordBatch;
use datafusion::physical_plan::SendableRecordBatchStream;
use futures::future::{BoxFuture, Future};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Stream adapter that wraps a RecordBatchStream and adds RowId information
/// Transforms each batch on-the-fly without collecting into memory
///
/// Uses a RowIdProvider trait to generate RowIds for each batch.
/// Different implementations can use different strategies:
/// - Pre-loaded from a layout file with caching (CSV)
/// - Computed on-the-fly based on file metadata (Parquet)
/// - Fetched from an external index service
pub struct RowIdStreamAdapter {
    inner: SendableRecordBatchStream,
    row_id_provider: Arc<dyn RowIdProvider>,
    global_row_num: usize,
    // Pending state: we're waiting for the RowId generation to complete
    pending: Option<(
        BoxFuture<'static, Result<Vec<RowId>, BundlebaseError>>,
        RecordBatch,
        usize,
    )>,
}

impl RowIdStreamAdapter {
    /// Create a new RowIdStreamAdapter
    ///
    /// # Arguments
    /// * `inner` - The RecordBatchStream to wrap
    /// * `row_id_provider` - Provider that generates RowIds for each batch
    pub fn new(inner: SendableRecordBatchStream, row_id_provider: Arc<dyn RowIdProvider>) -> Self {
        Self {
            inner,
            row_id_provider,
            global_row_num: 0,
            pending: None,
        }
    }
}

impl futures::stream::Stream for RowIdStreamAdapter {
    type Item = Result<RowIdBatch, BundlebaseError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // If we have a pending RowId generation, poll it first
        if let Some((mut fut, batch, start_row)) = self.pending.take() {
            match fut.as_mut().poll(cx) {
                Poll::Ready(Ok(batch_row_ids)) => {
                    // RowIds are ready, return the batch
                    self.global_row_num = start_row + batch.num_rows();
                    return Poll::Ready(Some(Ok(RowIdBatch::new(batch, batch_row_ids))));
                }
                Poll::Ready(Err(e)) => {
                    return Poll::Ready(Some(Err(e)));
                }
                Poll::Pending => {
                    // Still waiting for RowIds, put it back
                    self.pending = Some((fut, batch, start_row));
                    return Poll::Pending;
                }
            }
        }

        // No pending work, poll the inner stream for the next batch
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(batch))) => {
                let num_rows = batch.num_rows();
                let start_row = self.global_row_num;

                // Call the trait method to get RowIds
                let provider = self.row_id_provider.clone();
                let mut fut =
                    Box::pin(
                        async move { provider.get_row_ids(start_row, start_row + num_rows).await },
                    );

                // Try to poll it immediately (might complete synchronously if cached)
                match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(batch_row_ids)) => {
                        // Completed immediately (likely from cache)
                        self.global_row_num = start_row + num_rows;
                        Poll::Ready(Some(Ok(RowIdBatch::new(batch, batch_row_ids))))
                    }
                    Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
                    Poll::Pending => {
                        // Need to wait for it, store as pending
                        self.pending = Some((fut, batch, start_row));
                        Poll::Pending
                    }
                }
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(Box::new(e) as BundlebaseError))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
