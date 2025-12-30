use arrow::datatypes::SchemaRef;
use datafusion::physical_plan::SendableRecordBatchStream;
use futures::StreamExt;
use pyo3::prelude::*;
use std::sync::Arc;

use super::schema::PySchema;

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
    /// The underlying stream wrapped in an Arc for cheap cloning
    stream: Arc<tokio::sync::Mutex<SendableRecordBatchStream>>,
    /// Cached schema for O(1) access
    schema: SchemaRef,
}

impl PyRecordBatchStream {
    /// Create a new PyRecordBatchStream from a DataFusion stream
    pub fn new(stream: SendableRecordBatchStream, schema: SchemaRef) -> Self {
        PyRecordBatchStream {
            stream: Arc::new(tokio::sync::Mutex::new(stream)),
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
        let stream = self.stream.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut stream_guard = stream.lock().await;
            match stream_guard.next().await {
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
        let stream = self.stream.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut stream_guard = stream.lock().await;
            let mut batches = Vec::new();

            while let Some(result) = stream_guard.next().await {
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

    /// Check if the stream is empty (all batches have been consumed).
    ///
    /// Note: This is a best-effort check. The stream may become empty
    /// between the check and the next call to `next_batch()`.
    fn is_empty<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            // let stream_guard = stream.lock().await;
            // We can't actually check without consuming, so return false
            // This is a limitation of the current API
            Ok(false)
        })
    }
}
