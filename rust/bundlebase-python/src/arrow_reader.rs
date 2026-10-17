//! Binding-specific async-to-blocking bridge for the Arrow C Stream interface.
//!
//! Each Arrow request polls one batch on the existing Python binding runtime.
//! There is no producer, read-ahead queue, or dependence on an asyncio loop.
use crate::record_batch_stream::StreamState;
use arrow::datatypes::SchemaRef;
use arrow::error::ArrowError;
use arrow::record_batch::{RecordBatch, RecordBatchReader};
use futures::StreamExt;
use parking_lot::Mutex;
use pyo3::Python;
use std::sync::Arc;
use tokio::runtime::{Handle, RuntimeFlavor};

type PendingState = Arc<Mutex<Option<StreamState>>>;

pub(crate) struct BlockingArrowReader {
    schema: SchemaRef,
    state: PendingState,
    runtime: Handle,
}

impl BlockingArrowReader {
    pub(crate) fn new(schema: SchemaRef, state: PendingState) -> Self {
        Self {
            schema,
            state,
            runtime: pyo3_async_runtimes::tokio::get_runtime().handle().clone(),
        }
    }

    fn next_native(&mut self) -> Option<Result<RecordBatch, ArrowError>> {
        let mut state = self.state.lock().take()?;
        if let Some(batch) = state.peeked.take() {
            *self.state.lock() = Some(state);
            return Some(Ok(batch));
        }
        let poll = async move {
            let next = state.stream.next().await;
            (state, next)
        };
        let (state, next) = match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                // Consumers can call from Rust runtime threads as well as Python
                // threads. Let Tokio replace this worker while we block.
                tokio::task::block_in_place(|| self.runtime.block_on(poll))
            }
            Ok(_) => {
                // block_in_place is unavailable on a current-thread runtime.
                // Poll on the independent shared binding runtime instead. The
                // rendezvous has no read-ahead and creates no thread per reader.
                let (sender, receiver) = std::sync::mpsc::sync_channel(0);
                self.runtime.spawn(async move {
                    let _ = sender.send(poll.await);
                });
                match receiver.recv() {
                    Ok(result) => result,
                    Err(error) => {
                        return Some(Err(ArrowError::ExternalError(Box::new(error))));
                    }
                }
            }
            Err(_) => self.runtime.block_on(poll),
        };
        match next {
            Some(Ok(batch)) => {
                *self.state.lock() = Some(state);
                Some(Ok(batch))
            }
            Some(Err(error)) => Some(Err(ArrowError::ExternalError(Box::new(error)))),
            None => None,
        }
        // EOF and errors drop the query state immediately. Previously returned
        // batches own their Arrow buffers independently of this reader.
    }
}

impl Iterator for BlockingArrowReader {
    type Item = Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        // C Stream callbacks bypass PyO3 wrappers. Some consumers release the
        // GIL themselves and some retain it. Attach then detach handles both,
        // allowing Python UDFs/connectors to acquire it while execution waits.
        Python::attach(|py| py.detach(|| self.next_native()))
    }
}

impl RecordBatchReader for BlockingArrowReader {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Int64Array;
    use arrow::datatypes::{DataType, Field, Schema};
    use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn reader(
        count: usize,
        error: bool,
    ) -> (BlockingArrowReader, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let polls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        struct DropProbe(Arc<AtomicUsize>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let probe = DropProbe(drops.clone());
        let observed = polls.clone();
        let output_schema = schema.clone();
        let mut remaining = count;
        let stream = futures::stream::poll_fn(move |_| {
            let _ = &probe;
            observed.fetch_add(1, Ordering::SeqCst);
            if remaining == 0 {
                return std::task::Poll::Ready(None);
            }
            remaining -= 1;
            if error {
                return std::task::Poll::Ready(Some(Err(
                    datafusion::error::DataFusionError::Execution("bridge test failure".into()),
                )));
            }
            let batch = RecordBatch::try_new(
                output_schema.clone(),
                vec![Arc::new(Int64Array::from(vec![remaining as i64]))],
            );
            std::task::Poll::Ready(Some(batch.map_err(Into::into)))
        });
        let stream = Box::pin(RecordBatchStreamAdapter::new(schema.clone(), stream));
        let state = Arc::new(Mutex::new(Some(StreamState {
            stream,
            peeked: None,
        })));
        (BlockingArrowReader::new(schema, state), polls, drops)
    }

    /// Delay every underlying read with a Tokio timer created on the polling
    /// runtime. Count actual Pending polls so the reentrancy tests verify wakeups,
    /// rather than merely running an immediately ready stream inside Tokio.
    fn delayed_reader() -> (BlockingArrowReader, Arc<AtomicUsize>) {
        let (reader, _, _) = reader(1, false);
        let state = reader.state.lock().take().expect("active stream");
        let pending = Arc::new(AtomicUsize::new(0));
        let observed = pending.clone();
        let stream = futures::stream::unfold(state, move |mut state| {
            let observed = observed.clone();
            async move {
                use std::future::Future;
                let mut timer = Box::pin(tokio::time::sleep(std::time::Duration::from_millis(10)));
                futures::future::poll_fn(|context| {
                    let result = timer.as_mut().poll(context);
                    if result.is_pending() {
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                    result
                })
                .await;
                state.stream.next().await.map(|result| (result, state))
            }
        });
        let stream = Box::pin(RecordBatchStreamAdapter::new(reader.schema(), stream));
        *reader.state.lock() = Some(StreamState {
            stream,
            peeked: None,
        });
        (reader, pending)
    }

    #[test]
    fn timer_wakeup_outside_runtime() {
        let (mut reader, pending) = delayed_reader();
        assert!(reader.next_native().expect("batch").is_ok());
        assert!(pending.load(Ordering::SeqCst) > 0);
        assert!(reader.next_native().is_none());
    }

    #[test]
    fn later_error_releases_resources_and_preserves_returned_batch() {
        let (mut reader, _, drops) = reader(1, false);
        let (failing, _, error_drops) = self::reader(1, true);
        let success = reader.state.lock().take().expect("active stream");
        let failure = failing.state.lock().take().expect("active error stream");
        let stream = Box::pin(RecordBatchStreamAdapter::new(
            reader.schema(),
            success.stream.chain(failure.stream),
        ));
        *reader.state.lock() = Some(StreamState {
            stream,
            peeked: None,
        });
        let batch = reader.next_native().expect("batch").expect("valid batch");
        assert!(reader
            .next_native()
            .expect("error result")
            .expect_err("later error")
            .to_string()
            .contains("bridge test failure"));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(error_drops.load(Ordering::SeqCst), 1);
        assert_eq!(batch.num_rows(), 1);
        assert!(reader.next_native().is_none());
    }

    #[test]
    fn demand_schema_and_early_drop() {
        let (mut reader, polls, drops) = reader(100, false);
        assert_eq!(reader.schema().field(0).data_type(), &DataType::Int64);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        let batch = reader.next_native().expect("batch").expect("valid batch");
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        drop(reader);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(batch.num_rows(), 1);
    }

    #[test]
    fn typed_empty_and_error_release_resources() {
        let (mut empty, polls, drops) = reader(0, false);
        assert_eq!(empty.schema().fields().len(), 1);
        assert!(empty.next_native().is_none());
        assert!(empty.next_native().is_none());
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        let (mut failing, _, drops) = reader(1, true);
        assert!(failing
            .next_native()
            .expect("error result")
            .expect_err("stream fails")
            .to_string()
            .contains("bridge test failure"));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(failing.next_native().is_none());
    }

    #[test]
    fn peek_is_returned_before_polling() {
        let (mut reader, polls, _) = reader(1, false);
        let batch =
            RecordBatch::try_new(reader.schema(), vec![Arc::new(Int64Array::from(vec![42]))])
                .expect("valid batch");
        reader.state.lock().as_mut().expect("active stream").peeked = Some(batch);
        let batch = reader.next_native().expect("peek").expect("valid peek");
        assert_eq!(
            batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("int column")
                .value(0),
            42
        );
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert!(reader.next_native().is_some());
        assert_eq!(polls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runtime_worker_reentrancy() {
        let (mut reader, pending) = delayed_reader();
        assert!(reader.next_native().expect("batch").is_ok());
        assert!(pending.load(Ordering::SeqCst) > 0);
        assert!(reader.next_native().is_none());
    }

    #[tokio::test]
    async fn current_thread_runtime_reentrancy() {
        let (mut reader, pending) = delayed_reader();
        assert!(reader.next_native().expect("batch").is_ok());
        assert!(pending.load(Ordering::SeqCst) > 0);
        assert!(reader.next_native().is_none());
    }
}
