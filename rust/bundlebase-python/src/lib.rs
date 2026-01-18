mod builder;
mod bundle;
mod bundle_config;
mod commit;
mod data_generator;
mod function_impl;
mod operation;
mod progress;
mod record_batch_stream;
mod schema;
mod session_context;
mod utils;

use ::bundlebase::bundle::{Bundle, BundleBuilder};
use ::bundlebase::metrics::init_logging_metrics;
use pyo3::prelude::*;
use pyo3::types::PyModule;

use builder::{PyBundleBuilder, PyBundleStatus, PyChange, PyFetchedBlock, PyFetchResults};
use bundle::{PyBundle, PyFileVerificationResult, PyVerificationResults};
use bundle_config::{config_from_python, PyBundleConfig};
use commit::PyCommit;
use operation::PyOperation;
use record_batch_stream::PyRecordBatchStream;
use schema::{PySchema, PySchemaField};
use session_context::PySessionContext;

#[pyfunction]
#[pyo3(signature = (data_dir, config=None))]
pub fn create<'py>(
    data_dir: String,
    config: Option<&Bound<'py, PyAny>>,
    py: Python<'py>,
) -> PyResult<Bound<'py, PyAny>> {
    let config_inner = config.map(|c| config_from_python(c)).transpose()?;

    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        BundleBuilder::create(data_dir.as_str(), config_inner)
            .await
            .map(|o| PyBundleBuilder::new(o))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
    })
}

#[pyfunction]
#[pyo3(signature = (url, config=None))]
pub fn open<'py>(
    url: String,
    config: Option<&Bound<'py, PyAny>>,
    py: Python<'py>,
) -> PyResult<Bound<'py, PyAny>> {
    let config_inner = config.map(|c| config_from_python(c)).transpose()?;

    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        Bundle::open(url.as_str(), config_inner)
            .await
            .map(|o| PyBundle::new(o))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
    })
}

#[pyfunction]
fn log_metrics() {
    init_logging_metrics();
}

/// Get memory URL for test data file
#[pyfunction]
pub fn test_datafile(name: String) -> PyResult<String> {
    Ok(::bundlebase::test_utils::test_datafile(&name).to_string())
}

/// Get random memory URL for test bundle
#[pyfunction]
pub fn random_memory_url() -> PyResult<String> {
    Ok(::bundlebase::test_utils::random_memory_url().to_string())
}

#[pymodule(name = "_bundlebase")]
fn bundlebase(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(create, m)?)?;
    m.add_function(wrap_pyfunction!(log_metrics, m)?)?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(test_datafile, m)?)?;
    m.add_function(wrap_pyfunction!(random_memory_url, m)?)?;
    m.add_class::<PyBundle>()?;
    m.add_class::<PyBundleBuilder>()?;
    m.add_class::<PyChange>()?;
    m.add_class::<PyBundleStatus>()?;
    m.add_class::<PyFetchedBlock>()?;
    m.add_class::<PyFetchResults>()?;
    m.add_class::<PyBundleConfig>()?;
    m.add_class::<PySchema>()?;
    m.add_class::<PySchemaField>()?;
    m.add_class::<PyCommit>()?;
    m.add_class::<PyOperation>()?;
    m.add_class::<PyRecordBatchStream>()?;
    m.add_class::<PySessionContext>()?;
    m.add_class::<PyFileVerificationResult>()?;
    m.add_class::<PyVerificationResults>()?;

    // Initialize Rust→Python logging bridge
    // This forwards all Rust log::* calls to Python's logging module
    pyo3_log::init();

    // Register progress tracking functions
    progress::register_module(m)?;

    Ok(())
}
