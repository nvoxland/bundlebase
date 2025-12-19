mod builder;
mod commit;
mod bundle;
mod data_generator;
mod function_impl;
mod operation;
mod progress;
mod record_batch_stream;
mod schema;
mod session_context;
mod utils;

use crate::bundle::{Bundle, BundleBuilder};
use pyo3::prelude::*;
use pyo3::types::PyModule;

use builder::{PyBundleBuilder, PyChange, PyBundleStatus};
use commit::PyCommit;
use bundle::PyBundle;
use operation::PyOperation;
use record_batch_stream::PyRecordBatchStream;
use schema::{PySchema, PySchemaField};
use session_context::PySessionContext;

#[pyfunction]
pub fn create(data_dir: String, py: Python) -> PyResult<Bound<PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        BundleBuilder::create(data_dir.as_str())
            .await
            .map(|o| PyBundleBuilder::new(o))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
    })
}

#[pyfunction]
pub fn open(url: String, py: Python) -> PyResult<Bound<PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        Bundle::open(url.as_str())
            .await
            .map(|o| PyBundle::new(o))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
    })
}

/// Get memory URL for test data file
#[pyfunction]
pub fn test_datafile(name: String) -> PyResult<String> {
    Ok(crate::test_utils::test_datafile(&name).to_string())
}

/// Get random memory URL for test bundle
#[pyfunction]
pub fn random_memory_url() -> PyResult<String> {
    Ok(crate::test_utils::random_memory_url().to_string())
}

#[pymodule(name = "_bundlebase")]
fn bundlebase(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(create, m)?)?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(test_datafile, m)?)?;
    m.add_function(wrap_pyfunction!(random_memory_url, m)?)?;
    m.add_class::<PyBundle>()?;
    m.add_class::<PyBundleBuilder>()?;
    m.add_class::<PyChange>()?;
    m.add_class::<PyBundleStatus>()?;
    m.add_class::<PySchema>()?;
    m.add_class::<PySchemaField>()?;
    m.add_class::<PyCommit>()?;
    m.add_class::<PyOperation>()?;
    m.add_class::<PyRecordBatchStream>()?;
    m.add_class::<PySessionContext>()?;

    // Initialize Rust→Python logging bridge
    // This forwards all Rust log::* calls to Python's logging module
    pyo3_log::init();

    // Register progress tracking functions
    progress::register_module(m)?;

    Ok(())
}
