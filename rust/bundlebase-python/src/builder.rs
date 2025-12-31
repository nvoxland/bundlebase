use crate::function_impl::PythonFunctionImpl;
use crate::utils::convert_py_params;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use ::bundlebase::bundle::BundleBuilder;
use ::bundlebase::bundle::{BundleChange, BundleFacade, BundleStatus, JoinTypeOption};
use ::bundlebase::functions::FunctionSignature;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyFunction};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::commit::PyCommit;

#[pyclass]
#[derive(Clone)]
pub struct PyChange {
    #[pyo3(get)]
    id: String,
    #[pyo3(get)]
    description: String,
    #[pyo3(get)]
    operation_count: usize,
}

impl PyChange {
    pub fn from_rust(change: &BundleChange) -> Self {
        PyChange {
            id: change.id.to_string(),
            description: change.description.clone(),
            operation_count: change.operations.len(),
        }
    }
}

/// Bundle status showing uncommitted changes.
#[pyclass]
#[derive(Clone)]
pub struct PyBundleStatus {
    #[pyo3(get)]
    changes: Vec<PyChange>,
    #[pyo3(get)]
    change_count: usize,
    #[pyo3(get)]
    total_operations: usize,
}

#[pymethods]
impl PyBundleStatus {
    /// Check if there are any uncommitted changes
    fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Get a string representation of the status
    fn __str__(&self) -> String {
        self.to_string()
    }

    /// Get a debug representation of the status
    fn __repr__(&self) -> String {
        format!("PyBundleStatus({})", self.to_string())
    }
}

impl PyBundleStatus {
    fn from_rust(status: &BundleStatus) -> Self {
        let changes: Vec<PyChange> = status.changes().iter().map(PyChange::from_rust).collect();
        let change_count = changes.len();
        let total_operations = status.operations_count();

        PyBundleStatus {
            changes,
            change_count,
            total_operations,
        }
    }
}

impl std::fmt::Display for PyBundleStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            write!(f, "No uncommitted changes")
        } else {
            write!(
                f,
                "Bundle Status: {} change(s), {} total operation(s)",
                self.change_count, self.total_operations
            )?;
            for (idx, change) in self.changes.iter().enumerate() {
                write!(
                    f,
                    "\n  [{}] {} ({} operation{})",
                    idx + 1,
                    change.description,
                    change.operation_count,
                    if change.operation_count == 1 { "" } else { "s" }
                )?;
            }
            Ok(())
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyBundleBuilder {
    inner: Arc<Mutex<BundleBuilder>>,
}

/// Helper function to create a PyErr with operation context
fn to_py_error<E: std::fmt::Display>(context: &str, err: E) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("{}: {}", context, err))
}

#[pymethods]
impl PyBundleBuilder {
    #[getter]
    fn id(&self) -> Option<String> {
        self.inner
            .try_lock()
            .ok()
            .map(|builder| builder.bundle.id().to_string())
    }

    #[getter]
    fn name(&self) -> Option<String> {
        self.inner
            .try_lock()
            .ok()
            .and_then(|builder| builder.bundle.name().map(|s| s.to_string()))
    }

    /// Set the bundle name. Mutates the bundle in place.
    fn set_name<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .set_name(name.as_str())
                .await
                .map_err(|e| to_py_error(&format!("Failed to set bundle name '{}'", name), e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    #[getter]
    fn description(&self) -> Option<String> {
        self.inner
            .try_lock()
            .ok()
            .and_then(|builder| builder.bundle.description().map(|s| s.to_string()))
    }

    /// Set the bundle description. Mutates the bundle in place and returns it for chaining.
    fn set_description<'py>(
        slf: PyRef<'_, Self>,
        description: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let description = description.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .set_description(description.as_str())
                .await
                .map_err(|e| to_py_error("Failed to set bundle description", e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    /// Set a configuration value. Mutates the bundle in place and returns it for chaining.
    #[pyo3(signature = (key, value, url_prefix=None))]
    fn set_config<'py>(
        slf: PyRef<'_, Self>,
        key: &str,
        value: &str,
        url_prefix: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let key = key.to_string();
        let value = value.to_string();
        let url_prefix = url_prefix.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .set_config(key.as_str(), value.as_str(), url_prefix.as_deref())
                .await
                .map_err(|e| {
                    to_py_error(&format!("Failed to set config '{}' = '{}'", key, value), e)
                })?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    #[pyo3(signature = (name, output, func, version))]
    fn define_function<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        output: Py<PyDict>,
        func: Py<PyFunction>,
        version: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let version = version.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let schema: Vec<Field> = Python::attach(|py| {
                output
                    .bind_borrowed(py)
                    .iter()
                    .map(|(k, v)| {
                        let key = k.extract::<String>().map_err(|_| {
                            PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                                "Function output schema keys must be strings".to_string(),
                            )
                        })?;
                        let dtype_str = v.extract::<String>().map_err(|_| {
                            PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                                "Function output schema values must be strings".to_string(),
                            )
                        })?;
                        let dtype = DataType::from_str(&dtype_str).map_err(|e| {
                            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                                "Invalid data type '{}': {}",
                                dtype_str, e
                            ))
                        })?;
                        Ok(Field::new(key, dtype, true))
                    })
                    .collect::<PyResult<Vec<Field>>>()
            })?;

            let mut builder = inner.lock().await;
            builder
                .define_function(FunctionSignature::new(
                    name.as_str(),
                    SchemaRef::new(Schema::new(schema)),
                ))
                .await
                .map_err(|e| to_py_error(&format!("Failed to define function '{}'", name), e))?;

            builder
                .set_impl(
                    name.as_str(),
                    Arc::new(PythonFunctionImpl::new(func, version)),
                )
                .await
                .map_err(|e| {
                    to_py_error(
                        &format!("Failed to set implementation for function '{}'", name),
                        e,
                    )
                })?;
            drop(builder);

            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    fn attach<'py>(
        slf: PyRef<'_, Self>,
        url: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let url = url.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .attach(url.as_str())
                .await
                .map_err(|e| to_py_error(&format!("Failed to attach '{}'", url), e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    fn remove_column<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .remove_column(name.as_str())
                .await
                .map_err(|e| to_py_error(&format!("Failed to remove column '{}'", name), e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    fn rename_column<'py>(
        slf: PyRef<'_, Self>,
        old_name: &str,
        new_name: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let old_name = old_name.to_string();
        let new_name = new_name.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .rename_column(old_name.as_str(), new_name.as_str())
                .await
                .map_err(|e| {
                    to_py_error(
                        &format!("Failed to rename column '{}' to '{}'", old_name, new_name),
                        e,
                    )
                })?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    #[pyo3(signature = (name, url, expression, join_type=None))]
    fn join<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        url: &str,
        expression: &str,
        join_type: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let url = url.to_string();
        let expression = expression.to_string();
        let join_type = join_type.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let join_type_option = join_type
                .as_ref()
                .map(|jt| JoinTypeOption::from_str(jt))
                .unwrap_or(JoinTypeOption::Inner);

            let mut builder = inner.lock().await;
            builder
                .join(
                    name.as_str(),
                    url.as_str(),
                    expression.as_str(),
                    join_type_option,
                )
                .await
                .map_err(|e| to_py_error(&format!("Failed to join with '{}'", url), e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    fn attach_to_join<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        url: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let url = url.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .attach_to_join(name.as_str(), url.as_str())
                .await
                .map_err(|e| {
                    to_py_error(&format!("Failed to attach join source from '{}'", url), e)
                })?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    #[doc = "Returns a reference to the underlying PyArrow record batches for manual conversion to pandas, polars, numpy, etc."]
    fn as_pyarrow<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let builder = inner.lock().await;

            let df_future = builder.bundle.dataframe();
            let dataframe = df_future
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

            let dataframe = (*dataframe).clone();
            let record_batches = dataframe
                .collect()
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
            // Convert to PyArrow using the ToPyArrow trait with the Python GIL context
            use arrow::pyarrow::ToPyArrow;
            Python::attach(|py| -> PyResult<pyo3::Py<pyo3::PyAny>> {
                record_batches.to_pyarrow(py).map(|obj| obj.unbind())
            })
        })
    }

    #[doc = "Returns a streaming PyRecordBatchStream for processing large datasets without loading everything into memory."]
    fn as_pyarrow_stream<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let builder = inner.lock().await;

            let df_future = builder.bundle.dataframe();
            let dataframe = df_future
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

    #[pyo3(signature = (sql, params=None))]
    fn select<'py>(
        slf: PyRef<'_, Self>,
        sql: &str,
        params: Option<Vec<Py<PyAny>>>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let sql = sql.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let params_vec = if let Some(params_list) = params {
                convert_py_params(params_list)?
            } else {
                vec![]
            };

            let builder = inner.lock().await;
            let modified_bundle = builder
                .select(sql.as_str(), params_vec)
                .await
                .map_err(|e| to_py_error("Failed to execute query", e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: Arc::new(Mutex::new(modified_bundle)),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    #[pyo3(signature = (where_clause, params=None))]
    fn filter<'py>(
        slf: PyRef<'_, Self>,
        where_clause: &str,
        params: Option<Vec<Py<PyAny>>>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let where_clause = where_clause.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let params_vec = if let Some(params_list) = params {
                convert_py_params(params_list)?
            } else {
                vec![]
            };

            let mut builder = inner.lock().await;
            builder
                .filter(where_clause.as_str(), params_vec)
                .await
                .map_err(|e| to_py_error("Failed to apply filter", e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    fn num_rows<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let builder = inner.lock().await;

            let num_rows_future = builder.bundle.num_rows();
            num_rows_future
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
        })
    }

    /// Get the schema
    fn schema<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let builder = inner.lock().await;

            let schema_future = builder.bundle.schema();
            let schema = schema_future
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

            Python::attach(|py| {
                Py::new(py, super::schema::PySchema::new(schema)).map(|obj| obj.into_any())
            })
        })
    }

    fn commit<'py>(&self, message: &str, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let message = message.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder.commit(&message).await.map_err(|e| {
                to_py_error(&format!("Failed to commit with message '{}'", message), e)
            })?;
            Ok(())
        })
    }

    /// Reset all uncommitted operations, reverting to the last committed state.
    fn reset<'py>(slf: PyRef<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .reset()
                .await
                .map_err(|e| to_py_error("Failed to reset uncommitted operations", e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    /// Undo the last uncommitted operation.
    fn undo<'py>(slf: PyRef<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .undo()
                .await
                .map_err(|e| to_py_error("Failed to undo last operation", e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    fn explain<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let builder = inner.lock().await;

            let explain_future = builder.bundle.explain();
            explain_future
                .await
                .map_err(|e| to_py_error("Failed to explain query", e))
        })
    }

    #[getter]
    fn version(&self) -> String {
        self.inner
            .try_lock()
            .ok()
            .map(|builder| builder.bundle.version())
            .unwrap_or_default()
    }

    fn history(&self) -> Vec<PyCommit> {
        self.inner
            .try_lock()
            .ok()
            .and_then(|builder| {
                Some(
                    builder
                        .bundle
                        .history()
                        .into_iter()
                        .map(|commit| PyCommit::new(commit))
                        .collect(),
                )
            })
            .unwrap_or_default()
    }

    #[getter]
    fn url(&self) -> String {
        self.inner
            .try_lock()
            .ok()
            .map(|builder| builder.bundle.url().to_string())
            .unwrap_or_default()
    }

    /// Create an index on the specified column for optimized lookups
    fn index<'py>(
        slf: PyRef<'_, Self>,
        column: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let column = column.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder.index(&column).await.map_err(|e| {
                to_py_error(&format!("Failed to create index on column '{}'", column), e)
            })?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    /// Rebuild an index on the specified column
    fn rebuild_index<'py>(
        slf: PyRef<'_, Self>,
        column: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let column = column.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder.rebuild_index(&column).await.map_err(|e| {
                to_py_error(
                    &format!("Failed to rebuild index on column '{}'", column),
                    e,
                )
            })?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    /// Drop an index on the specified column
    fn drop_index<'py>(
        slf: PyRef<'_, Self>,
        column: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let column = column.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder.drop_index(&column).await.map_err(|e| {
                to_py_error(&format!("Failed to drop index on column '{}'", column), e)
            })?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    /// Reindex - create or update index files for columns that are missing them
    ///
    /// This method ensures all blocks have index files for columns that have been
    /// defined as indexed. It checks existing indexes to avoid redundant work and
    /// continues with other columns if one fails (logs warnings).
    fn reindex<'py>(slf: PyRef<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .reindex()
                .await
                .map_err(|e| to_py_error("Failed to reindex", e))?;
            drop(builder);
            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    fn ctx<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let builder = inner.lock().await;

            let ctx = builder.bundle.ctx();

            Python::attach(|py| {
                Py::new(py, super::session_context::PySessionContext::new(ctx))
                    .map(|obj| obj.into_any())
            })
        })
    }

    /// Get the bundle status showing uncommitted changes.
    fn status(&self) -> PyBundleStatus {
        self.inner
            .try_lock()
            .ok()
            .map(|builder| PyBundleStatus::from_rust(&builder.status()))
            .unwrap_or_else(|| PyBundleStatus {
                changes: vec![],
                change_count: 0,
                total_operations: 0,
            })
    }

    /// Attach a view from another BundleBuilder
    fn create_view<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        source: PyRef<'_, PyBundleBuilder>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let source_inner = source.inner.clone();
        let name = name.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            // Clone the source builder first to avoid deadlock if source == self
            // The Rust create_view will clone it anyway (builder.rs:483)
            let source_builder_clone = {
                let source_guard = source_inner.lock().await;
                source_guard.clone()
            };

            let mut builder = inner.lock().await;
            builder
                .create_view(&name, &source_builder_clone)
                .await
                .map_err(|e| to_py_error(&format!("Failed to create view '{}'", name), e))?;

            drop(builder);

            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    /// Rename an existing view
    fn rename_view<'py>(
        slf: PyRef<'_, Self>,
        old_name: &str,
        new_name: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let old_name = old_name.to_string();
        let new_name = new_name.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .rename_view(old_name.as_str(), new_name.as_str())
                .await
                .map_err(|e| to_py_error(&format!("Failed to rename view '{}'", old_name), e))?;

            drop(builder);

            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    /// Drop an existing view
    fn drop_view<'py>(
        slf: PyRef<'_, Self>,
        view_name: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let view_name = view_name.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .drop_view(view_name.as_str())
                .await
                .map_err(|e| to_py_error(&format!("Failed to drop view '{}'", view_name), e))?;

            drop(builder);

            Python::attach(|py| {
                Py::new(
                    py,
                    PyBundleBuilder {
                        inner: inner.clone(),
                    },
                )
                .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    /// Open a view by name or ID, returning a read-only Bundle
    fn view<'py>(
        slf: PyRef<'_, Self>,
        identifier: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let identifier = identifier.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let builder = inner.lock().await;
            let bundle = builder
                .view(&identifier)
                .await
                .map_err(|e| to_py_error(&format!("Failed to open view '{}'", identifier), e))?;
            drop(builder);

            Python::attach(|py| {
                Py::new(py, super::bundle::PyBundle::new(bundle))
                    .map_err(|e| to_py_error("Failed to create bundle", e))
            })
        })
    }

    fn views(&self) -> HashMap<String, String> {
        Python::with_gil(|_py| {
            self.inner
                .blocking_lock()
                .views()
                .into_iter()
                .map(|(id, name)| (id.to_string(), name))
                .collect()
        })
    }

    fn operations(&self) -> Vec<super::operation::PyOperation> {
        Python::with_gil(|_py| {
            self.inner
                .blocking_lock()
                .bundle()
                .operations()
                .iter()
                .map(|op| super::operation::PyOperation::new(op.clone()))
                .collect()
        })
    }
}

impl PyBundleBuilder {
    pub fn new(inner: BundleBuilder) -> Self {
        PyBundleBuilder {
            inner: Arc::new(Mutex::new(inner)),
        }
    }
}
