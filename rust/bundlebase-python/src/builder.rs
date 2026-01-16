use crate::function_impl::PythonFunctionImpl;
use crate::utils::convert_py_params;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use ::bundlebase::bundle::BundleBuilder;
use ::bundlebase::bundle::{BundleChange, BundleFacade, BundleStatus};
use ::bundlebase::functions::FunctionSignature;
use ::bundlebase::source::{FetchedBlock, FetchResults};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyFunction};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;
use bundlebase::bundle::JoinTypeOption;
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

/// Information about a block that was fetched (added or replaced).
#[pyclass]
#[derive(Clone)]
pub struct PyFetchedBlock {
    /// Location where the block is attached (path in data_dir or URL)
    #[pyo3(get)]
    pub attach_location: String,
    /// Original source location identifier
    #[pyo3(get)]
    pub source_location: String,
}

impl PyFetchedBlock {
    pub fn from_rust(block: &FetchedBlock) -> Self {
        PyFetchedBlock {
            attach_location: block.attach_location.clone(),
            source_location: block.source_location.clone(),
        }
    }
}

#[pymethods]
impl PyFetchedBlock {
    fn __repr__(&self) -> String {
        format!(
            "FetchedBlock(attach_location='{}', source_location='{}')",
            self.attach_location, self.source_location
        )
    }
}

/// Results from fetching a single source.
#[pyclass]
#[derive(Clone)]
pub struct PyFetchResults {
    /// Source function name (e.g., "remote_dir", "web_scrape")
    #[pyo3(get)]
    pub source_function: String,
    /// Source URL or identifier
    #[pyo3(get)]
    pub source_url: String,
    /// Pack name ("base" or join name)
    #[pyo3(get)]
    pub pack: String,
    /// Blocks that were newly added
    #[pyo3(get)]
    pub added: Vec<PyFetchedBlock>,
    /// Blocks that were replaced (updated)
    #[pyo3(get)]
    pub replaced: Vec<PyFetchedBlock>,
    /// Source locations of blocks that were removed
    #[pyo3(get)]
    pub removed: Vec<String>,
}

impl PyFetchResults {
    pub fn from_rust(results: &FetchResults) -> Self {
        PyFetchResults {
            source_function: results.source_function.clone(),
            source_url: results.source_url.clone(),
            pack: results.pack.clone(),
            added: results.added.iter().map(PyFetchedBlock::from_rust).collect(),
            replaced: results.replaced.iter().map(PyFetchedBlock::from_rust).collect(),
            removed: results.removed.clone(),
        }
    }
}

#[pymethods]
impl PyFetchResults {
    /// Total number of actions (added + replaced + removed).
    fn total_count(&self) -> usize {
        self.added.len() + self.replaced.len() + self.removed.len()
    }

    /// Check if there were any changes.
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.replaced.is_empty() && self.removed.is_empty()
    }

    fn __repr__(&self) -> String {
        format!(
            "FetchResults(source_function='{}', source_url='{}', pack='{}', added={}, replaced={}, removed={})",
            self.source_function,
            self.source_url,
            self.pack,
            self.added.len(),
            self.replaced.len(),
            self.removed.len()
        )
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
    fn create_function<'py>(
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
                .create_function(FunctionSignature::new(
                    name.as_str(),
                    SchemaRef::new(Schema::new(schema)),
                ))
                .await
                .map_err(|e| to_py_error(&format!("Failed to create function '{}'", name), e))?;

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

    #[pyo3(signature = (location, pack="base"))]
    fn attach<'py>(
        slf: PyRef<'_, Self>,
        location: &str,
        pack: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let location = location.to_string();
        let pack = if pack == "base" {
            None
        } else {
            Some(pack.to_string())
        };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .attach(location.as_str(), pack.as_deref())
                .await
                .map_err(|e| to_py_error(&format!("Failed to attach '{}'", location), e))?;
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

    /// Detach a data block from the bundle by its location.
    ///
    /// Removes a previously attached block from the bundle. The block is
    /// identified by its location (URL).
    ///
    /// # Arguments
    /// * `location` - The location (URL) of the block to detach
    ///
    /// # Example
    /// ```python
    /// bundle = await bundle.detach_block("s3://bucket/data.parquet")
    /// ```
    fn detach_block<'py>(
        slf: PyRef<'_, Self>,
        location: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let location = location.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .detach_block(location.as_str())
                .await
                .map_err(|e| to_py_error(&format!("Failed to detach block at '{}'", location), e))?;
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

    /// Replace a block's data location in the bundle.
    ///
    /// Changes where a block's data is read from without changing the block's
    /// identity. Useful when data files are moved to a new location.
    ///
    /// # Arguments
    /// * `old_location` - The current location (URL) of the block
    /// * `new_location` - The new location (URL) to read data from
    ///
    /// # Example
    /// ```python
    /// bundle = await bundle.replace_block(
    ///     "s3://old-bucket/data.parquet",
    ///     "s3://new-bucket/data.parquet"
    /// )
    /// ```
    fn replace_block<'py>(
        slf: PyRef<'_, Self>,
        old_location: &str,
        new_location: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let old_location = old_location.to_string();
        let new_location = new_location.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .replace_block(old_location.as_str(), new_location.as_str())
                .await
                .map_err(|e| {
                    to_py_error(
                        &format!("Failed to replace block '{}' -> '{}'", old_location, new_location),
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

    fn drop_column<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .drop_column(name.as_str())
                .await
                .map_err(|e| to_py_error(&format!("Failed to drop column '{}'", name), e))?;
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

    #[pyo3(signature = (name, expression, location=None, join_type=None))]
    fn join<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        expression: &str,
        location: Option<&str>,
        join_type: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let location = location.map(|s| s.to_string());
        let expression = expression.to_string();
        let join_type = join_type.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let join_type_option = match &join_type {
                None => JoinTypeOption::Inner,
                Some(jt) => {
                    let jt_lower = jt.to_lowercase();
                    match jt_lower.as_str() {
                        "inner" => JoinTypeOption::Inner,
                        "left" => JoinTypeOption::Left,
                        "right" => JoinTypeOption::Right,
                        "full" => JoinTypeOption::Full,
                        _ => {
                            return Err(to_py_error(
                                "Invalid join_type",
                                format!(
                                    "'{}' is not a valid join type. Valid options: Inner, Left, Right, Full",
                                    jt
                                ),
                            ));
                        }
                    }
                }
            };

            let mut builder = inner.lock().await;
            builder
                .join(
                    name.as_str(),
                    expression.as_str(),
                    location.as_deref(),
                    join_type_option,
                )
                .await
                .map_err(|e| {
                    let msg = match &location {
                        Some(u) => format!("Failed to join with '{}'", u),
                        None => format!("Failed to create join point '{}'", name),
                    };
                    to_py_error(&msg, e)
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

    /// Create a data source for a pack.
    ///
    /// A source specifies where to look for data files (e.g., S3 bucket prefix)
    /// and patterns to filter which files to include.
    ///
    /// # Arguments
    /// * `function` - Source function name (e.g., "remote_dir")
    /// * `args` - Function-specific arguments. For "remote_dir":
    ///   - "url" (required): Directory URL to list (e.g., "s3://bucket/data/")
    ///   - "patterns" (optional): Comma-separated glob patterns (e.g., "**/*.parquet,**/*.csv")
    /// * `pack` - Which pack to create the source for:
    ///   - "base" (default): The base pack
    ///   - A join name: A joined pack by its join name
    #[pyo3(signature = (function, args, pack="base"))]
    fn create_source<'py>(
        slf: PyRef<'_, Self>,
        function: &str,
        args: HashMap<String, String>,
        pack: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let function = function.to_string();
        let pack = if pack == "base" {
            None
        } else {
            Some(pack.to_string())
        };
        let url = args.get("url").cloned().unwrap_or_else(|| "<no url>".to_string());
        let pack_name = pack.clone().unwrap_or_else(|| "base".to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .create_source(&function, args, pack.as_deref())
                .await
                .map_err(|e| {
                    to_py_error(
                        &format!("Failed to create source for {} at '{}'", pack_name, url),
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

    /// Fetch from sources for a pack - discover and attach new files.
    ///
    /// # Arguments
    /// * `pack` - Which pack to fetch sources for ("base" for base pack, or a join name)
    ///
    /// Returns a list of FetchResults, one for each source in the pack.
    #[pyo3(signature = (pack="base"))]
    fn fetch<'py>(
        slf: PyRef<'_, Self>,
        pack: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let pack = if pack == "base" {
            None
        } else {
            Some(pack.to_string())
        };
        let pack_name = pack.clone().unwrap_or_else(|| "base".to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            let results = builder
                .fetch(pack.as_deref())
                .await
                .map_err(|e| to_py_error(&format!("Failed to fetch from pack '{}'", pack_name), e))?;
            let py_results: Vec<PyFetchResults> = results.iter().map(PyFetchResults::from_rust).collect();
            Ok(py_results)
        })
    }

    /// Fetch from all defined sources - discover and attach new files.
    ///
    /// Returns a list of FetchResults, one for each source across all packs.
    fn fetch_all<'py>(slf: PyRef<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            let results = builder
                .fetch_all()
                .await
                .map_err(|e| to_py_error("Failed to fetch from sources", e))?;
            let py_results: Vec<PyFetchResults> = results.iter().map(PyFetchResults::from_rust).collect();
            Ok(py_results)
        })
    }

    /// Returns the underlying PyArrow record batches for manual conversion.
    ///
    /// WARNING: This method materializes the entire dataset into memory.
    /// For large datasets, use `as_pyarrow_stream()` instead which streams
    /// data in batches without loading everything into memory.
    ///
    /// Recommended alternatives:
    /// - `to_pandas()` / `to_polars()` - These stream internally and use constant memory
    /// - `as_pyarrow_stream()` - For custom incremental processing
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

    fn export_tar<'py>(
        slf: PyRef<'_, Self>,
        tar_path: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let tar_path = tar_path.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let builder = inner.lock().await;
            builder
                .export_tar(&tar_path)
                .await
                .map_err(|e| to_py_error(&format!("Failed to export to tar '{}'", tar_path), e))?;
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

    /// Drop an existing join
    fn drop_join<'py>(
        slf: PyRef<'_, Self>,
        join_name: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let join_name = join_name.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = inner.lock().await;
            builder
                .drop_join(join_name.as_str())
                .await
                .map_err(|e| to_py_error(&format!("Failed to drop join '{}'", join_name), e))?;

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

    /// Rename an existing join
    fn rename_join<'py>(
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
                .rename_join(old_name.as_str(), new_name.as_str())
                .await
                .map_err(|e| to_py_error(&format!("Failed to rename join '{}'", old_name), e))?;

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
        Python::attach(|_py| {
            self.inner
                .blocking_lock()
                .views()
                .into_iter()
                .map(|(id, name)| (id.to_string(), name))
                .collect()
        })
    }

    fn operations(&self) -> Vec<super::operation::PyOperation> {
        Python::attach(|_py| {
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
