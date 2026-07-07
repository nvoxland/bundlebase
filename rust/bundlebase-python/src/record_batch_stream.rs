use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use datafusion::physical_plan::SendableRecordBatchStream;
use futures::StreamExt;
use pyo3::prelude::*;
use std::sync::Arc;

use super::schema::PySchema;

/// The stream plus an optional one-batch look-ahead buffer used by `is_empty()`.
struct StreamState {
    stream: SendableRecordBatchStream,
    /// A batch already pulled by `is_empty()` that `next_batch()`/`collect_all()`
    /// must return before polling the stream again.
    peeked: Option<RecordBatch>,
}

/// Python-exposed wrapper around DataFusion's RecordBatch stream.
///
/// This class provides streaming access to record batches without materializing
/// the entire dataset in memory. It wraps DataFusion's `SendableRecordBatchStream`
/// and allows Python code to iterate through batches one at a time.
///
/// # Memory Efficiency
///
/// Unlike `as_pyarrow()` which collects all batches into memory, this stream
/// processes one batch at a time, maintaining constant memory usage regardless
/// of dataset size.
///
/// # Example (Python)
///
/// ```python
/// stream = await bundle.as_pyarrow_stream()
/// while True:
///     batch = await stream.next_batch()
///     if batch is None:
///         break
///     df_chunk = batch.to_pandas()
///     # Process chunk...
/// ```
#[pyclass]
pub struct PyRecordBatchStream {
    /// The underlying stream + look-ahead, wrapped in an Arc for cheap cloning
    inner: Arc<tokio::sync::Mutex<StreamState>>,
    /// Cached schema for O(1) access
    schema: SchemaRef,
}

impl PyRecordBatchStream {
    /// Create a new PyRecordBatchStream from a DataFusion stream
    pub fn new(stream: SendableRecordBatchStream, schema: SchemaRef) -> Self {
        PyRecordBatchStream {
            inner: Arc::new(tokio::sync::Mutex::new(StreamState {
                stream,
                peeked: None,
            })),
            schema,
        }
    }
}

#[pymethods]
impl PyRecordBatchStream {
    /// Get the next RecordBatch from the stream.
    ///
    /// # Returns
    ///
    /// * `Some(PyArrow RecordBatch)` - The next batch in the stream
    /// * `None` - Stream is exhausted
    ///
    /// # Example
    ///
    /// ```python
    /// while True:
    ///     batch = await stream.next_batch()
    ///     if batch is None:
    ///         break
    ///     process(batch)
    /// ```
    fn next_batch<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut guard = inner.lock().await;
            // Return the look-ahead batch (if is_empty() pulled one) before polling.
            let next = match guard.peeked.take() {
                Some(batch) => Some(Ok(batch)),
                None => guard.stream.next().await,
            };
            match next {
                Some(Ok(batch)) => {
                    // Convert the RecordBatch to PyArrow
                    use arrow::pyarrow::ToPyArrow;
                    Python::attach(|py| -> PyResult<Py<PyAny>> {
                        batch
                            .to_pyarrow(py)
                            .map(|obj: Bound<'_, PyAny>| obj.unbind())
                    })
                }
                Some(Err(e)) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Error reading batch: {}",
                    e
                ))),
                None => Python::attach(|py| Ok(py.None())),
            }
        })
    }

    /// Collect all remaining batches into a list.
    ///
    /// **Warning**: This defeats the purpose of streaming and may cause OOM
    /// for large datasets. Use only for small datasets or testing.
    ///
    /// # Returns
    ///
    /// List of PyArrow RecordBatches
    fn collect_all<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut stream_guard = inner.lock().await;
            let mut batches = Vec::new();
            if let Some(batch) = stream_guard.peeked.take() {
                batches.push(batch);
            }

            while let Some(result) = stream_guard.stream.next().await {
                let batch = result.map_err(|e: datafusion::error::DataFusionError| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                        "Error reading batch: {}",
                        e
                    ))
                })?;
                batches.push(batch);
            }

            // Convert all batches to PyArrow
            use arrow::pyarrow::ToPyArrow;
            Python::attach(|py| -> PyResult<Py<PyAny>> {
                batches
                    .to_pyarrow(py)
                    .map(|obj: Bound<'_, PyAny>| obj.unbind())
            })
        })
    }

    /// Get the schema of the stream.
    ///
    /// # Returns
    ///
    /// PySchema wrapper around the Arrow schema
    #[getter]
    fn schema(&self) -> PySchema {
        PySchema::new(self.schema.clone())
    }

    /// Check if the stream is exhausted.
    ///
    /// Pulls one batch into an internal look-ahead buffer if none is buffered
    /// yet; that batch is returned by the next `next_batch()`/`collect_all()`
    /// call, so peeking does not drop data. Returns `True` only once the
    /// underlying stream has actually ended.
    fn is_empty<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut guard = inner.lock().await;
            if guard.peeked.is_some() {
                return Ok(false);
            }
            match guard.stream.next().await {
                None => Ok(true),
                Some(Ok(batch)) => {
                    guard.peeked = Some(batch);
                    Ok(false)
                }
                Some(Err(e)) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                    "Error reading batch: {}",
                    e
                ))),
            }
        })
    }
}
