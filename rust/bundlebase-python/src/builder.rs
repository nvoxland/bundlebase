use crate::utils::convert_py_params;
use ::bundlebase::bundle::BundleBuilder;
use ::bundlebase::bundle::{BundleChange, BundleFacade, BundleStatus};
use ::bundlebase::source::{FetchedBlock, FetchResults, SyncMode};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
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
    /// Connector name (e.g., "remote_dir", "web_scrape")
    #[pyo3(get)]
    pub connector: String,
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
            connector: results.connector.clone(),
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
            "FetchResults(connector='{}', source_url='{}', pack='{}', added={}, replaced={}, removed={})",
            self.connector,
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
    inner: Arc<BundleBuilder>,
}

/// Helper function to create a PyErr with operation context
fn to_py_error<E: std::fmt::Display>(err: E) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(err.to_string())
}

/// Helper function to create a PyErr with operation context description
fn to_py_error_ctx<E: std::fmt::Display>(context: &str, err: E) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("{}: {}", context, err))
}

#[pymethods]
impl PyBundleBuilder {
    #[getter]
    fn id(&self) -> String {
        self.inner.bundle().id().to_string()
    }

    #[getter]
    fn name(&self) -> Option<String> {
        self.inner.bundle().name().map(|s| s.to_string())
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
            inner
                .set_name(name.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to set name", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    #[getter]
    fn description(&self) -> Option<String> {
        self.inner.bundle().description().map(|s| s.to_string())
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
            inner
                .set_description(description.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to set description", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    /// Save a configuration value to the bundle manifest. Mutates the bundle in place and returns it for chaining.
    ///
    /// # Arguments
    /// * `scope` - Scope ("" for global default, or path like "s3/bucket" or "prod")
    /// * `key` - Configuration key
    /// * `value` - Configuration value
    fn save_config<'py>(
        slf: PyRef<'_, Self>,
        scope: &str,
        key: &str,
        value: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let scope = super::bundle_config::parse_scope(scope)?;
        let inner = slf.inner.clone();
        let key = key.to_string();
        let value = value.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .save_config(
                    &scope,
                    key.as_str(),
                    value.as_str(),
                )
                .await
                .map_err(|e| to_py_error_ctx("Failed to save config", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    /// Set a runtime config value (session-only, highest priority).
    ///
    /// Unlike save_config, this does not persist the value to the bundle manifest.
    /// It only affects the current session.
    fn set_config<'py>(
        slf: PyRef<'_, Self>,
        scope: &str,
        key: &str,
        value: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let scope = super::bundle_config::parse_scope(scope)?;
        let inner = slf.inner.clone();
        let key = key.to_string();
        let value = value.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .set_config(&scope, &key, &value)
                .await
                .map_err(|e| to_py_error_ctx("Failed to set config", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    #[pyo3(signature = (name, input_types, return_type, runner, logic, platform="*/*", function_type="scalar"))]
    fn import_function<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        input_types: Vec<String>,
        return_type: &str,
        runner: &str,
        logic: &str,
        platform: &str,
        function_type: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let return_type = return_type.to_string();
        let runner = runner.to_string();
        let logic = logic.to_string();
        let platform = platform.to_string();
        let function_type = function_type.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .import_function(&name, input_types, &return_type, &runner, &logic, &platform, &function_type)
                .await
                .map_err(|e| to_py_error_ctx("Failed to load function", e))?;

            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    #[pyo3(signature = (name, input_types, return_type, runner, logic, platform="*/*", function_type="scalar"))]
    fn import_temp_function<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        input_types: Vec<String>,
        return_type: &str,
        runner: &str,
        logic: &str,
        platform: &str,
        function_type: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let return_type = return_type.to_string();
        let runner = runner.to_string();
        let logic = logic.to_string();
        let platform = platform.to_string();
        let function_type = function_type.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            use ::bundlebase::bundle::{Platform, Runner, FunctionEntry, FunctionKind};
            use ::bundlebase::bundle::parse_arrow_type_name;
            let runner: Runner = runner.parse().map_err(|e: ::bundlebase::BundlebaseError| to_py_error(e))?;
            let platform: Platform = platform.parse().map_err(|e: ::bundlebase::BundlebaseError| to_py_error(e))?;
            let kind: FunctionKind = function_type.parse().map_err(|e: ::bundlebase::BundlebaseError| to_py_error(e))?;
            let namespaced: ::bundlebase::NamespacedName = name.parse().map_err(|e: ::bundlebase::BundlebaseError| to_py_error(e))?;
            let parsed_input_types = input_types.iter()
                .map(|s| parse_arrow_type_name(s))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| to_py_error(e))?;
            let parsed_return_type = parse_arrow_type_name(&return_type)
                .map_err(|e| to_py_error(e))?;
            let entry = FunctionEntry {
                id: ::bundlebase::io::ObjectId::generate(),
                name: namespaced,
                input_types: parsed_input_types,
                return_type: parsed_return_type,
                runner,
                logic,
                platform,
                temporary: true,
                kind,
            };
            inner
                .as_ref()
                .import_temp_function(entry)
                .await
                .map_err(|e| to_py_error_ctx("Failed to load temporary function", e))?;

            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    #[pyo3(signature = (name, platform=None))]
    fn drop_function<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        platform: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let platform = platform.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .drop_function(&name, platform.as_deref())
                .await
                .map_err(|e| to_py_error_ctx("Failed to drop function", e))?;

            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner
                .attach(location.as_str(), pack.as_deref())
                .await
                .map_err(|e| to_py_error_ctx("Failed to attach", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner
                .detach_block(location.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to detach block", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner
                .replace_block(old_location.as_str(), new_location.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to replace block", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner
                .drop_column(name.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to drop column", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    #[pyo3(signature = (name, expression))]
    fn add_column<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        expression: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let expression = expression.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .add_column(&name, &expression)
                .await
                .map_err(|e| to_py_error_ctx("Failed to add column", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    #[pyo3(signature = (name, new_type, clean=None))]
    fn cast_column<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        new_type: &str,
        clean: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let new_type = new_type.to_string();
        let clean = clean.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .cast_column(&name, &new_type, clean)
                .await
                .map_err(|e| to_py_error_ctx("Failed to cast column", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    fn standardize_column_names<'py>(
        slf: PyRef<'_, Self>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .standardize_column_names()
                .await
                .map_err(|e| to_py_error_ctx("Failed to standardize column names", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner
                .rename_column(old_name.as_str(), new_name.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to rename column", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
                            return Err(to_py_error(format!(
                                    "'{}' is not a valid join type. Valid options: Inner, Left, Right, Full",
                                    jt
                                )));
                        }
                    }
                }
            };

            inner
                .join(
                    name.as_str(),
                    expression.as_str(),
                    location.as_deref(),
                    join_type_option,
                )
                .await
                .map_err(|e| to_py_error_ctx("Failed to create join", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    /// Create a data source for a pack.
    ///
    /// A source specifies where to look for data files (e.g., S3 bucket prefix)
    /// and patterns to filter which files to include.
    ///
    /// # Arguments
    /// * `connector` - Connector name (e.g., "remote_dir" for built-in, "acme.weather" for custom)
    /// * `args` - Connector-specific arguments. For "remote_dir":
    ///   - "url" (required): Directory URL to list (e.g., "s3://bucket/data/")
    ///   - "patterns" (optional): Comma-separated glob patterns (e.g., "**/*.parquet,**/*.csv")
    /// * `pack` - Which pack to create the source for:
    ///   - "base" (default): The base pack
    ///   - A join name: A joined pack by its join name
    #[pyo3(signature = (connector, args, pack="base"))]
    fn create_source<'py>(
        slf: PyRef<'_, Self>,
        connector: &str,
        args: HashMap<String, String>,
        pack: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let connector = connector.to_string();
        let pack = if pack == "base" {
            None
        } else {
            Some(pack.to_string())
        };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .create_source(&connector, args, pack.as_deref())
                .await
                .map_err(|e| to_py_error_ctx("Failed to create source", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    /// Load a named connector with logic (persisted).
    ///
    /// # Arguments
    /// * `name` - Dot-separated connector name (e.g., "acme.weather")
    /// * `runner` - The runner: "lib", "java", "docker", or "ipc"
    /// * `logic` - The logic string (path to shared library or binary)
    /// * `platform` - Docker-style platform string (e.g., "*/*", "linux/amd64")
    #[pyo3(signature = (name, runner, logic, platform="*/*"))]
    fn import_connector<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        runner: &str,
        logic: &str,
        platform: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let runner = runner.to_string();
        let logic = logic.to_string();
        let platform = platform.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .import_connector(&name, &runner, &logic, &platform)
                .await
                .map_err(|e| to_py_error_ctx("Failed to load connector", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    /// Load a temporary connector with runtime-only logic (not persisted).
    ///
    /// # Arguments
    /// * `name` - The connector name
    /// * `runner` - The runner: "python", "lib", "java", "docker", or "ipc"
    /// * `logic` - The logic string (e.g., "mod:Class" for python)
    /// * `platform` - Docker-style platform string (e.g., "*/*", "linux/amd64")
    #[pyo3(signature = (name, runner, logic, platform="*/*"))]
    fn import_temp_connector<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        runner: &str,
        logic: &str,
        platform: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let runner = runner.to_string();
        let logic = logic.to_string();
        let platform = platform.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let runner: ::bundlebase::bundle::Runner = runner.parse().map_err(|e: ::bundlebase::BundlebaseError| to_py_error(e))?;
            let platform: ::bundlebase::bundle::Platform = platform.parse().map_err(|e: ::bundlebase::BundlebaseError| to_py_error(e))?;
            inner
                .as_ref()
                .import_temp_connector(&name, runner, logic, platform)
                .await
                .map_err(|e| to_py_error_ctx("Failed to load temporary connector", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    /// Drop a connector. Without a platform, removes the entire definition.
    /// With a platform, removes only the logic for that platform.
    ///
    /// # Arguments
    /// * `name` - The dotted connector name (e.g., "acme.weather")
    /// * `platform` - Optional platform filter (e.g., "linux/amd64"). None drops entire connector.
    #[pyo3(signature = (name, platform=None))]
    fn drop_connector<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        platform: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let platform = platform.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .drop_connector(&name, platform.as_deref())
                .await
                .map_err(|e| to_py_error_ctx("Failed to drop connector", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    /// Drop runtime-only connector (session-only, no operation created).
    ///
    /// # Arguments
    /// * `name` - The dotted connector name (e.g., "acme.weather")
    /// * `platform` - Optional platform filter (e.g., "linux/amd64"). None drops all.
    ///
    /// Returns the number of entries removed.
    #[pyo3(signature = (name, platform=None))]
    fn drop_temp_connector<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        platform: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let platform = platform.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let count = inner
                .drop_temp_connector(&name, platform.as_deref())
                .await
                .map_err(|e| to_py_error_ctx("Failed to drop temporary connector", e))?;
            Ok(format!(
                "Dropped {} temporary connector logic entries for: {}",
                count, name
            ))
        })
    }

    /// Fetch from sources for a pack - discover and attach new files.
    ///
    /// # Arguments
    /// * `pack` - Which pack to fetch sources for ("base" for base pack, or a join name)
    /// * `mode` - Sync mode: "add", "update", or "sync".
    ///
    /// Returns a list of FetchResults, one for each source in the pack.
    fn fetch<'py>(
        slf: PyRef<'_, Self>,
        pack: &str,
        mode: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let pack = pack.to_string();
        let sync_mode = SyncMode::from_arg(mode).map_err(|e| to_py_error_ctx("Invalid sync mode", e))?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let results = inner
                .fetch(&pack, sync_mode)
                .await
                .map_err(|e| to_py_error_ctx("Failed to fetch", e))?;
            let py_results: Vec<PyFetchResults> = results.iter().map(PyFetchResults::from_rust).collect();
            Ok(py_results)
        })
    }

    /// Fetch from all defined sources - discover and attach new files.
    ///
    /// # Arguments
    /// * `mode` - Sync mode: "add", "update", or "sync".
    ///
    /// Returns a list of FetchResults, one for each source across all packs.
    fn fetch_all<'py>(slf: PyRef<'_, Self>, mode: &str, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let sync_mode = SyncMode::from_arg(mode).map_err(|e| to_py_error_ctx("Invalid sync mode", e))?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let results = inner
                .fetch_all(sync_mode)
                .await
                .map_err(|e| to_py_error_ctx("Failed to fetch all", e))?;
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
            let dataframe = inner.dataframe()
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
            let dataframe = inner.dataframe()
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

    #[pyo3(signature = (data_dir=None))]
    fn extend<'py>(
        slf: PyRef<'_, Self>,
        data_dir: Option<&str>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let data_dir_owned = data_dir.map(|s| s.to_string());
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let new_builder = inner.extend(data_dir_owned.as_deref()).await
                .map_err(|e| to_py_error_ctx("Failed to extend bundle", e))?;
            Ok(PyBundleBuilder { inner: new_builder })
        })
    }

    #[pyo3(signature = (sql, params=None))]
    fn query<'py>(
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

            let stream = inner
                .query(sql.as_str(), params_vec)
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to execute query: {}\n  SQL: {}",
                        e, sql
                    ))
                })?;

            let schema = std::sync::Arc::new(stream.schema().as_ref().clone());
            Python::attach(|py| {
                Py::new(
                    py,
                    super::record_batch_stream::PyRecordBatchStream::new(stream, schema),
                )
                .map_err(|e| to_py_error(e))
            })
        })
    }

    #[pyo3(signature = (query, params=None))]
    fn filter<'py>(
        slf: PyRef<'_, Self>,
        query: &str,
        params: Option<Vec<Py<PyAny>>>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let query = query.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let params_vec = if let Some(params_list) = params {
                convert_py_params(params_list)?
            } else {
                vec![]
            };

            inner
                .filter(query.as_str(), params_vec)
                .await
                .map_err(|e| to_py_error_ctx("Failed to apply filter", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    fn num_rows<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner.num_rows()
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
        })
    }

    /// Get the schema
    fn schema<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let schema = inner.schema()
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
            inner.commit(&message).await.map_err(|e| to_py_error_ctx("Failed to commit", e))?;
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
            inner
                .export_tar(&tar_path)
                .await
                .map_err(|e| to_py_error_ctx("Failed to export tar", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    /// Reset all uncommitted operations, reverting to the last committed state.
    fn reset<'py>(slf: PyRef<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .reset()
                .await
                .map_err(|e| to_py_error_ctx("Failed to reset", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    /// Undo the last uncommitted operation.
    fn undo<'py>(slf: PyRef<'_, Self>, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .undo()
                .await
                .map_err(|e| to_py_error_ctx("Failed to undo", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
                .map_err(|e| to_py_error_ctx("Failed to explain", e))?;
            let schema = std::sync::Arc::new(stream.schema().as_ref().clone());
            Python::attach(|py| {
                Py::new(
                    py,
                    super::record_batch_stream::PyRecordBatchStream::new(stream, schema),
                )
                .map_err(|e| to_py_error(e))
            })
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
                .map_err(|e| to_py_error_ctx("Failed to describe function", e))?;
            let schema = std::sync::Arc::new(stream.schema().as_ref().clone());
            Python::attach(|py| {
                Py::new(
                    py,
                    super::record_batch_stream::PyRecordBatchStream::new(stream, schema),
                )
                .map_err(|e| to_py_error(e))
            })
        })
    }

    #[getter]
    fn version(&self) -> String {
        self.inner.version()
    }

    fn history(&self) -> Vec<PyCommit> {
        self.inner
            .bundle()
            .history()
            .into_iter()
            .map(|commit| PyCommit::new(commit))
            .collect()
    }

    #[getter]
    fn url(&self) -> String {
        self.inner.bundle().url().to_string()
    }

    /// Create an index on the specified column(s) for optimized lookups
    ///
    /// # Arguments
    /// * `column` - Column name (str) or list of column names (list[str])
    /// * `index_type` - Index type: "column" or "text"
    /// * `args` - Optional index-specific arguments (e.g., {"tokenizer": "en_stem"} for text indexes)
    /// * `name` - Optional index name (for text indexes). If not provided, auto-generated as idx_{columns}
    ///
    /// # Example
    /// ```python
    /// # Column index
    /// c = await c.create_index("id", "column")
    ///
    /// # Text index — single column, auto-named "idx_description"
    /// c = await c.create_index("description", "text")
    ///
    /// # Text index — multiple columns, auto-named "idx_title_description"
    /// c = await c.create_index(["title", "description"], "text")
    ///
    /// # Text index — explicit name
    /// c = await c.create_index(["title", "description"], "text", name="product_search")
    ///
    /// # Text index — with tokenizer
    /// c = await c.create_index("content", "text", args={"tokenizer": "en_stem"})
    /// ```
    #[pyo3(signature = (columns, index_type, args=None, name=None))]
    fn create_index<'py>(
        slf: PyRef<'_, Self>,
        columns: &Bound<'py, PyAny>,
        index_type: &str,
        args: Option<HashMap<String, String>>,
        name: Option<String>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        use pyo3::exceptions::PyValueError;

        let inner = slf.inner.clone();

        // Accept either a single string or a list of strings
        let columns: Vec<String> = if let Ok(s) = columns.extract::<String>() {
            vec![s]
        } else if let Ok(list) = columns.extract::<Vec<String>>() {
            list
        } else {
            return Err(PyValueError::new_err(
                "columns must be a string or list of strings"
            ));
        };

        // Build the IndexType
        let mut configured_type = bundlebase::IndexType::from_str(index_type)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;

        // Apply args (e.g., tokenizer)
        configured_type = configured_type
            .with_args(&args.unwrap_or_default())
            .map_err(|e| PyValueError::new_err(e.to_string()))?;

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let col_refs: Vec<&str> = columns.iter().map(|s| s.as_str()).collect();
            inner
                .create_index(&col_refs, configured_type, name.as_deref())
                .await
                .map_err(|e| to_py_error_ctx("Failed to create index", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner.rebuild_index(&column).await.map_err(|e| to_py_error_ctx("Failed to rebuild index", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner.drop_index(&column).await.map_err(|e| to_py_error_ctx("Failed to drop index", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner
                .reindex()
                .await
                .map_err(|e| to_py_error_ctx("Failed to reindex", e))?;
            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
            })
        })
    }

    fn ctx<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let ctx = inner.bundle().ctx();

            Python::attach(|py| {
                Py::new(py, super::session_context::PySessionContext::new(ctx))
                    .map(|obj| obj.into_any())
            })
        })
    }

    /// Get the bundle status showing uncommitted changes.
    fn status(&self) -> PyBundleStatus {
        PyBundleStatus::from_rust(&self.inner.status())
    }

    /// Create a view from a SQL statement
    ///
    /// # Arguments
    /// * `name` - The name of the view
    /// * `sql` - SQL query that defines the view (e.g., "SELECT * FROM bundle WHERE age > 21")
    ///
    /// # Returns
    /// The BundleBuilder for the created view
    fn create_view<'py>(
        slf: PyRef<'_, Self>,
        name: &str,
        sql: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();
        let name = name.to_string();
        let sql = sql.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let view_builder = inner
                .create_view(&name, &sql)
                .await
                .map_err(|e| to_py_error_ctx("Failed to create view", e))?;

            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner: view_builder })
                    .map_err(|e| to_py_error(e))
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
            inner
                .rename_view(old_name.as_str(), new_name.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to rename view", e))?;

            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner
                .drop_view(view_name.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to drop view", e))?;

            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner
                .drop_join(join_name.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to drop join", e))?;

            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            inner
                .rename_join(old_name.as_str(), new_name.as_str())
                .await
                .map_err(|e| to_py_error_ctx("Failed to rename join", e))?;

            Python::attach(|py| {
                Py::new(py, PyBundleBuilder { inner })
                    .map_err(|e| to_py_error(e))
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
            let bundle = inner
                .view(&identifier)
                .await
                .map_err(|e| to_py_error_ctx("Failed to open view", e))?;

            Python::attach(|py| {
                Py::new(py, super::bundle::PyBundle::new(bundle))
                    .map_err(|e| to_py_error(e))
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

    fn operations(&self) -> Vec<super::operation::PyOperation> {
        self.inner
            .bundle()
            .operations()
            .iter()
            .map(|op| super::operation::PyOperation::new(op.clone()))
            .collect()
    }

    /// Verify the integrity of all files in the bundle by checking SHA256 hashes.
    ///
    /// # Arguments
    /// * `update_versions` - If true and hash matches but version changed, add UpdateVersionOp
    ///   to update stored version metadata. Defaults to false.
    ///
    /// Returns VerificationResults with details for each file verified.
    #[pyo3(signature = (update_versions=false))]
    fn verify_data<'py>(
        &self,
        update_versions: bool,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let results = inner
                .verify_data(update_versions)
                .await
                .map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to verify data: {}",
                        e
                    ))
                })?;
            Python::attach(|py| {
                Py::new(py, super::bundle::PyVerificationResults::from(&results))
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

impl PyBundleBuilder {
    pub fn new(inner: Arc<BundleBuilder>) -> Self {
        PyBundleBuilder { inner }
    }
}
