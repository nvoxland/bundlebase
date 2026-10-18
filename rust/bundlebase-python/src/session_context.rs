use arrow::pyarrow::ToPyArrow;
use datafusion::prelude::SessionContext;
use pyo3::prelude::*;
use std::sync::Arc;

#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PySessionContext {
    inner: Arc<SessionContext>,
}

#[pymethods]
impl PySessionContext {
    fn sql<'py>(&self, py: Python<'py>, query: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let dataframe =
                inner
                    .sql(&query)
                    .await
                    .map_err(|e: datafusion::error::DataFusionError| {
                        PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
                    })?;

            // Stream execution (ADR-003) and accumulate only the final
            // result batches — collect() materializes ~3x the dataset in
            // DataFusion's intermediate state (bb-cko.3). The return contract
            // (a list of pyarrow batches) is unchanged.
            let mut stream = dataframe.execute_stream().await.map_err(
                |e: datafusion::error::DataFusionError| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
                },
            )?;
            let mut record_batches = Vec::new();
            while let Some(batch) = futures::StreamExt::next(&mut stream).await {
                record_batches.push(batch.map_err(|e: datafusion::error::DataFusionError| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
                })?);
            }

            Python::attach(|py| -> PyResult<Py<PyAny>> {
                record_batches
                    .to_pyarrow(py)
                    .map(|obj: Bound<'_, PyAny>| obj.unbind())
            })
        })
    }
}

impl PySessionContext {
    pub fn new(inner: Arc<SessionContext>) -> Self {
        PySessionContext { inner }
    }
}
