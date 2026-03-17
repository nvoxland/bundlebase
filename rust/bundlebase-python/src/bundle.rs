use super::commit::PyCommit;
use arrow::pyarrow::ToPyArrow;
use ::bundlebase::bundle::BundleFacade;
use ::bundlebase::{Bundle, FileVerificationResult, VerificationResults};
use pyo3::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

/// Result of verifying a single file
#[pyclass]
#[derive(Clone)]
pub struct PyFileVerificationResult {
    #[pyo3(get)]
    pub location: String,
    #[pyo3(get)]
    pub file_type: String,
    #[pyo3(get)]
    pub expected_hash: Option<String>,
    #[pyo3(get)]
    pub actual_hash: Option<String>,
    #[pyo3(get)]
    pub passed: bool,
    #[pyo3(get)]
    pub error: Option<String>,
    #[pyo3(get)]
    pub version_updated: bool,
}

impl From<&FileVerificationResult> for PyFileVerificationResult {
    fn from(result: &FileVerificationResult) -> Self {
        Self {
            location: result.location.clone(),
            file_type: result.file_type.clone(),
            expected_hash: result.expected_hash.clone(),
            actual_hash: result.actual_hash.clone(),
            passed: result.passed,
            error: result.error.clone(),
            version_updated: result.version_updated,
        }
    }
}

#[pymethods]
impl PyFileVerificationResult {
    fn __repr__(&self) -> String {
        let status = if self.passed { "passed" } else { "FAILED" };
        format!(
            "FileVerificationResult(location='{}', type='{}', status={})",
            self.location, self.file_type, status
        )
    }
}

/// Complete verification results for a bundle
#[pyclass]
#[derive(Clone)]
pub struct PyVerificationResults {
    #[pyo3(get)]
    pub files: Vec<PyFileVerificationResult>,
    #[pyo3(get)]
    pub passed_count: usize,
    #[pyo3(get)]
    pub failed_count: usize,
    #[pyo3(get)]
    pub skipped_count: usize,
    #[pyo3(get)]
    pub versions_updated_count: usize,
    #[pyo3(get)]
    pub all_passed: bool,
}

impl From<&VerificationResults> for PyVerificationResults {
    fn from(results: &VerificationResults) -> Self {
        Self {
            files: results.files.iter().map(PyFileVerificationResult::from).collect(),
            passed_count: results.passed_count,
            failed_count: results.failed_count,
            skipped_count: results.skipped_count,
            versions_updated_count: results.versions_updated_count,
            all_passed: results.all_passed,
        }
    }
}

#[pymethods]
impl PyVerificationResults {
    /// Check verification results and raise exception if any files failed.
    fn check(&self) -> PyResult<()> {
        if self.all_passed {
            Ok(())
        } else {
            let failures: Vec<&PyFileVerificationResult> =
                self.files.iter().filter(|f| !f.passed).collect();

            let messages: Vec<String> = failures
                .iter()
                .map(|f| {
                    if let Some(ref err) = f.error {
                        format!("{}: {}", f.location, err)
                    } else if f.expected_hash != f.actual_hash {
                        format!(
                            "{}: hash mismatch (expected {}, got {})",
                            f.location,
                            f.expected_hash.as_deref().unwrap_or("none"),
                            f.actual_hash.as_deref().unwrap_or("none")
                        )
                    } else {
                        format!("{}: verification failed", f.location)
                    }
                })
                .collect();

            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "Data verification failed for {} file(s):\n{}",
                failures.len(),
                messages.join("\n")
            )))
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "VerificationResults(passed={}, failed={}, skipped={}, versions_updated={})",
            self.passed_count, self.failed_count, self.skipped_count, self.versions_updated_count
        )
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyBundle {
    inner: Arc<Bundle>,
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

    #[pyo3(signature = (verbose=false, analyze=false, format=None, sql=None))]
    fn explain<'py>(
        &self,
        verbose: bool,
        analyze: bool,
        format: Option<&str>,
        sql: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let format_str = format.map(|s| s.to_string());
        let sql_str = sql.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let explain_format = match format_str.as_deref() {
                Some("tree") | Some("TREE") => datafusion::logical_expr::ExplainFormat::Tree,
                Some("graphviz") | Some("GRAPHVIZ") => {
                    datafusion::logical_expr::ExplainFormat::Graphviz
                }
                _ => datafusion::logical_expr::ExplainFormat::Indent,
            };
            let stream = inner
                .explain(verbose, analyze, explain_format, sql_str.as_deref())
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let schema = std::sync::Arc::new(stream.schema().as_ref().clone());
            Python::attach(|py| {
                Py::new(
                    py,
                    super::record_batch_stream::PyRecordBatchStream::new(stream, schema),
                )
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                        "Failed to create stream: {}",
                        e
                    ))
                })
            })
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
    fn extend<'py>(
        &self,
        py: Python<'py>,
        data_dir: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let data_dir_owned = data_dir.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let builder = inner.extend(data_dir_owned.as_deref()).await.map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Failed to extend bundle: {}",
                    e
                ))
            })?;
            Ok(super::builder::PyBundleBuilder::new(builder))
        })
    }

    #[pyo3(signature = (sql, params=None))]
    fn query<'py>(
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

            let stream = inner
                .query(&sql, params_vec)
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to execute query: {}\n  SQL: {}",
                        e, sql
                    ))
                })?;

            let schema = std::sync::Arc::new(stream.schema().as_ref().clone());
            Python::attach(|py| {
                Py::new(py, super::record_batch_stream::PyRecordBatchStream::new(stream, schema))
                    .map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to create stream: {}",
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

    /// Set a runtime config value (session-only, highest priority).
    ///
    /// Unlike save_config, this does not persist the value to the bundle manifest.
    /// It only affects the current session.
    fn set_config<'py>(
        &self,
        scope: &str,
        key: &str,
        value: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let scope = super::bundle_config::parse_scope(scope)?;
        let inner = self.inner.clone();
        let key = key.to_string();
        let value = value.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .set_config(&scope, &key, &value)
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to set config '{}': {}",
                        key, e
                    ))
                })?;
            Ok(format!("OK: SET CONFIG {}", key))
        })
    }

    /// Describe a registered function's metadata.
    ///
    /// Returns a record batch stream with columns: name, kind, input_types,
    /// return_type, runner, logic, platform, temporary.
    fn describe_function<'py>(
        &self,
        name: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let sql = format!("DESCRIBE FUNCTION {}", name);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let stream = inner
                .query(&sql, vec![])
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to describe function: {}",
                        e
                    ))
                })?;
            let schema = std::sync::Arc::new(stream.schema().as_ref().clone());
            Python::attach(|py| {
                Py::new(
                    py,
                    super::record_batch_stream::PyRecordBatchStream::new(stream, schema),
                )
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                        "Failed to create stream: {}",
                        e
                    ))
                })
            })
        })
    }

    /// Set temporary (runtime-only) connector logic for a connector.
    ///
    /// Load a temporary connector with runtime-only logic (not persisted).
    #[pyo3(signature = (name, from_, platform="*/*"))]
    fn import_temp_connector<'py>(
        &self,
        name: &str,
        from_: &str,
        platform: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let name = name.to_string();
        let from_ = from_.to_string();
        let platform = platform.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let from = ::bundlebase::bundle::LogicRuntime::parse_from(&from_).map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
            })?;
            from.validate_logic().map_err(|e| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Failed to load temporary connector: {}", e
                ))
            })?;
            let platform: ::bundlebase::bundle::Platform = platform.parse().map_err(|e: ::bundlebase::BundlebaseError| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
            })?;
            let namespaced: ::bundlebase::NamespacedName = name.parse().map_err(|e: ::bundlebase::BundlebaseError| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
            })?;
            let entry = ::bundlebase::bundle::ConnectorEntry {
                id: ::bundlebase::io::ObjectId::generate(),
                name: namespaced,
                from,
                platform,
                temporary: true,
            };
            inner
                .import_temp_connector(entry)
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to load temporary connector: {}",
                        e
                    ))
                })?;
            Ok(format!("Loaded temporary connector: {}", name))
        })
    }

    /// Load a temporary function with runtime-only logic (not persisted).
    ///
    /// Types and kind are auto-detected from the function's manifest.
    #[pyo3(signature = (name, from_, platform="*/*"))]
    fn import_temp_function<'py>(
        &self,
        name: &str,
        from_: &str,
        platform: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let name = name.to_string();
        let from_ = from_.to_string();
        let platform = platform.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            use ::bundlebase::bundle::{Platform, ImportTempFunctionCommand};
            use ::bundlebase::bundle::BundleFacadeCommand;
            let platform: Platform = platform.parse().map_err(|e: ::bundlebase::BundlebaseError| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
            })?;
            let cmd = ImportTempFunctionCommand::new(&name, &from_, platform);
            Box::new(cmd)
                .execute(inner.as_ref())
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to load temporary function: {}",
                        e
                    ))
                })
        })
    }

    /// Drop runtime-only connector (session-only).
    ///
    /// # Arguments
    /// * `name` - The dotted connector name (e.g., "acme.weather")
    /// * `platform` - Optional platform filter (e.g., "linux/amd64"). None drops all.
    ///
    /// Returns a message describing what was dropped.
    #[pyo3(signature = (name, platform=None))]
    fn drop_temp_connector<'py>(
        &self,
        name: &str,
        platform: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let name = name.to_string();
        let platform: Option<::bundlebase::bundle::Platform> = platform
            .map(|s| s.parse())
            .transpose()
            .map_err(|e: ::bundlebase::BundlebaseError| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
            })?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let count = inner
                .drop_temp_connector(&name, platform.as_ref())
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to drop temporary connector: {}",
                        e
                    ))
                })?;
            Ok(format!(
                "Dropped {} temporary connector logic entries for: {}",
                count, name
            ))
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

    /// Verify the integrity of all files in the bundle by checking SHA256 hashes.
    ///
    /// Returns VerificationResults with details for each file verified.
    fn verify_data<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let results = inner
                .verify_data()
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to verify data: {}",
                        e
                    ))
                })?;
            Python::attach(|py| {
                Py::new(py, PyVerificationResults::from(&results))
                    .map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                            "Failed to create verification results: {}",
                            e
                        ))
                    })
            })
        })
    }
}

impl PyBundle {
    pub fn new(inner: Arc<Bundle>) -> Self {
        PyBundle { inner }
    }
}
