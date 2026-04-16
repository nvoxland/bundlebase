use crate::{RowId, RowIdBatch};
use bundlebase_common::object_id::ObjectIdAlias;
use bundlebase_common::BundlebaseError;
use datafusion::physical_plan::SendableRecordBatchStream;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Stream adapter that wraps a RecordBatchStream and adds sequential RowId
/// information to each batch.
///
/// Generates logical RowIds on-the-fly: each row gets a sequential row number
/// combined with the provided `block_ref`. No layout file or external provider
/// is needed — RowIds are purely logical identifiers.
pub struct RowIdStreamAdapter {
    inner: SendableRecordBatchStream,
    block_ref: ObjectIdAlias,
    global_row_num: u32,
}

impl RowIdStreamAdapter {
    /// Create a new RowIdStreamAdapter.
    ///
    /// # Arguments
    /// * `inner` - The RecordBatchStream to wrap
    /// * `block_ref` - The ObjectIdAlias to embed in each RowId
    pub fn new(inner: SendableRecordBatchStream, block_ref: ObjectIdAlias) -> Self {
        Self {
            inner,
            block_ref,
            global_row_num: 0,
        }
    }
}

impl futures::stream::Stream for RowIdStreamAdapter {
    type Item = Result<RowIdBatch, BundlebaseError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(batch))) => {
                let num_rows = batch.num_rows();
                let mut row_ids = Vec::with_capacity(num_rows);

                for _ in 0..num_rows {
                    row_ids.push(RowId::new(self.block_ref, self.global_row_num));
                    self.global_row_num = match self.global_row_num.checked_add(1) {
                        Some(n) => n,
                        None => {
                            return Poll::Ready(Some(Err(
                                "Row count exceeds u32::MAX (~4 billion rows)".into(),
                            )))
                        }
                    };
                }

                Poll::Ready(Some(RowIdBatch::new(batch, row_ids)))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(Box::new(e) as BundlebaseError))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
