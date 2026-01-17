use super::commit::PyCommit;
use arrow::pyarrow::ToPyArrow;
use ::bundlebase::bundle::BundleFacade;
use ::bundlebase::Bundle;
use pyo3::prelude::*;
use std::collections::HashMap;

#[pyclass]
#[derive(Clone)]
pub struct PyBundle {
    inner: Bundle,
}

#[pymethods]
impl PyBundle {
    #[getter]
    fn id(&self) -> String {
        self.inner.id().to_string()
    }

    #[getter]
    fn name(&self) -> Option<String> {
        self.inner.name().map(|s| s.to_string())
    }

    #[getter]
    fn description(&self) -> Option<String> {
        self.inner.description().map(|s| s.to_string())
    }

    #[doc = "Returns a reference to the underlying PyArrow record batches for manual conversion to pandas, polars, numpy, etc."]
    fn as_pyarrow<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let dataframe = inner
                .dataframe()
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let dataframe = (*dataframe).clone();
            let record_batches = dataframe
                .collect()
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
            Python::attach(|py| -> PyResult<Py<PyAny>> {
                record_batches.to_pyarrow(py).map(|obj| obj.unbind())
            })
        })
    }

    #[doc = "Returns a streaming PyRecordBatchStream for processing large datasets without loading everything into memory."]
    fn as_pyarrow_stream<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let dataframe = inner
                .dataframe()
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let dataframe = (*dataframe).clone();

            // Convert DFSchema to Arrow Schema
            let schema = std::sync::Arc::new(dataframe.schema().as_arrow().clone());

            // Execute as stream instead of collecting all batches
            let stream = dataframe
                .execute_stream()
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;

            Python::attach(|py| {
                Py::new(
                    py,
                    super::record_batch_stream::PyRecordBatchStream::new(stream, schema),
                )
            })
        })
    }

    fn num_rows<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .num_rows()
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
        })
    }

    fn schema<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let schema = inner
                .schema()
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            Python::attach(|py| {
                Py::new(py, super::schema::PySchema::new(schema)).map(|obj| obj.into_any())
            })
        })
    }

    fn explain<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .explain()
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
        })
    }

    #[getter]
    fn version(&self) -> String {
        self.inner.version()
    }

    fn history(&self) -> Vec<PyCommit> {
        self.inner
            .history()
            .into_iter()
            .map(|commit| PyCommit::new(commit))
            .collect()
    }

    #[getter]
    fn url(&self) -> String {
        self.inner.url().to_string()
    }

    #[pyo3(signature = (data_dir=None))]
    fn extend(&self, data_dir: Option<&str>) -> PyResult<super::builder::PyBundleBuilder> {
        let builder = self.inner.extend(data_dir).map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Failed to extend bundle: {}",
                e
            ))
        })?;
        Ok(super::builder::PyBundleBuilder::new(builder))
    }

    #[pyo3(signature = (sql, params=None))]
    fn select<'py>(
        &self,
        sql: &str,
        params: Option<Vec<Py<PyAny>>>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let sql = sql.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let params_vec = if let Some(params_list) = params {
                super::utils::convert_py_params(params_list)?
            } else {
                vec![]
            };

            let builder = inner
                .select(&sql, params_vec)
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to execute query: {}",
                        e
                    ))
                })?;

            Python::attach(|py| {
                Py::new(py, super::builder::PyBundleBuilder::new(builder))
                    .map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to create bundle: {}",
                            e
                        ))
                    })
            })
        })
    }

    fn ctx<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let ctx = inner.ctx();
            Python::attach(|py| {
                Py::new(py, super::session_context::PySessionContext::new(ctx))
                    .map(|obj| obj.into_any())
            })
        })
    }

    fn views(&self) -> HashMap<String, String> {
        self.inner
            .views()
            .into_iter()
            .map(|(id, name)| (id.to_string(), name))
            .collect()
    }

    fn view<'py>(&self, identifier: &str, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let identifier = identifier.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let bundle = inner
                .view(&identifier)
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to open view '{}': {}",
                        identifier, e
                    ))
                })?;

            Python::attach(|py| {
                Py::new(py, PyBundle::new(bundle))
                    .map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to create bundle: {}",
                            e
                        ))
                    })
            })
        })
    }

    fn operations(&self) -> Vec<super::operation::PyOperation> {
        self.inner
            .operations()
            .iter()
            .map(|op| super::operation::PyOperation::new(op.clone()))
            .collect()
    }

    fn export_tar<'py>(
        &self,
        tar_path: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let tar_path = tar_path.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .export_tar(&tar_path)
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to export to tar '{}': {}",
                        tar_path, e
                    ))
                })
        })
    }
}

impl PyBundle {
    pub fn new(inner: Bundle) -> Self {
        PyBundle { inner }
    }
}
