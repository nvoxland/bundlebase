//! Semantic-layer read accessors shared by `PyBundle` and `PyBundleBuilder`:
//! column descriptions, metric definitions, metric evaluation and the
//! data-quality profile. Thin wrappers over `BundleFacade`.

use ::bundlebase::bundle::{BundleFacade, DataQualityReport};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::collections::HashMap;
use std::sync::Arc;

/// Preserve JSON value types while crossing the Python boundary.
pub(crate) fn value_to_py(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    let text = serde_json::to_string(value)
        .map_err(|e| value_error("Cannot serialize semantic metadata", e))?;
    Ok(py.import("json")?.call_method1("loads", (text,))?.unbind())
}

pub(crate) fn value_from_py(value: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    let kwargs = PyDict::new(value.py());
    kwargs.set_item("allow_nan", false)?;
    let text: String = value
        .py()
        .import("json")?
        .call_method("dumps", (value,), Some(&kwargs))?
        .extract()?;
    serde_json::from_str(&text).map_err(|e| value_error("Invalid semantic metadata", e))
}

pub(crate) fn semantic_model<'py>(
    py: Python<'py>,
    facade: Arc<dyn BundleFacade>,
) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let document = facade
            .semantic_model()
            .await
            .map_err(|e| value_error("Failed to project semantic model", e))?;
        Python::attach(|py| value_to_py(py, &document))
    })
}

pub(crate) fn export_semantic_model<'py>(
    py: Python<'py>,
    facade: Arc<dyn BundleFacade>,
    path: String,
) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        ::bundlebase::bundle::semantic_model::export_semantic_model(facade.as_ref(), &path)
            .await
            .map_err(|e| value_error("Failed to export semantic model", e))
    })
}

pub(crate) fn mutate<'py>(
    py: Python<'py>,
    inner: Arc<::bundlebase::BundleBuilder>,
    command: bundlebase_command::BundleCommand,
) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        command
            .execute(&inner)
            .await
            .map_err(|e| value_error("Failed to update semantic metadata", e))?;
        Python::attach(|py| Py::new(py, super::builder::PyBundleBuilder::new(inner)))
    })
}

pub(crate) fn execute<'py>(
    py: Python<'py>,
    facade: Arc<dyn BundleFacade>,
    sql: String,
    params: Option<Vec<Py<PyAny>>>,
) -> PyResult<Bound<'py, PyAny>> {
    use bundlebase_command::BundleFacadeCommandExt;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let params = super::utils::convert_py_params(params.unwrap_or_default())?;
        let stream = facade
            .execute(&sql, params)
            .await
            .map_err(|e| value_error("Failed to execute command", e))?;
        let schema = stream.schema();
        Python::attach(|py| {
            Py::new(
                py,
                super::record_batch_stream::PyRecordBatchStream::new(stream, schema),
            )
        })
    })
}

fn value_error<E: std::fmt::Display>(context: &str, e: E) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("{}: {}", context, e))
}

/// `[{"name", "expression", "description"}, ...]` in name order.
pub(crate) fn metrics(facade: &dyn BundleFacade) -> Vec<HashMap<String, Option<String>>> {
    facade
        .metrics()
        .into_iter()
        .map(|m| {
            HashMap::from([
                ("name".to_string(), Some(m.name)),
                ("expression".to_string(), Some(m.expression)),
                ("description".to_string(), m.description),
            ])
        })
        .collect()
}

/// `{column_name: description}` using current column names.
pub(crate) fn column_descriptions(facade: &dyn BundleFacade) -> HashMap<String, String> {
    facade.column_descriptions_by_name()
}

/// Awaitable resolving to a `PyRecordBatchStream` of the metric's rows.
pub(crate) fn compute_metric<'py>(
    py: Python<'py>,
    facade: Arc<dyn BundleFacade>,
    name: String,
    dimensions: Vec<String>,
    limit: Option<usize>,
) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let stream = facade
            .compute_metric(&name, &dimensions, limit)
            .await
            .map_err(|e| value_error("Failed to compute metric", e))?;
        let schema = stream.schema();
        Python::attach(|py| {
            Py::new(
                py,
                super::record_batch_stream::PyRecordBatchStream::new(stream, schema),
            )
        })
    })
}

/// Awaitable resolving to `{"total_rows": int, "columns": [dict, ...]}`.
pub(crate) fn data_quality<'py>(
    py: Python<'py>,
    facade: Arc<dyn BundleFacade>,
) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let report = facade
            .data_quality()
            .await
            .map_err(|e| value_error("Failed to compute data quality", e))?;
        Python::attach(|py| report_to_py(py, report))
    })
}

fn report_to_py(py: Python<'_>, report: DataQualityReport) -> PyResult<Py<PyAny>> {
    let columns = PyList::empty(py);
    for c in report.columns {
        let d = PyDict::new(py);
        d.set_item("name", c.name)?;
        d.set_item("non_null", c.non_null)?;
        d.set_item("null_count", c.null_count)?;
        d.set_item("null_rate", c.null_rate)?;
        d.set_item("distinct", c.distinct)?;
        d.set_item("all_null", c.all_null)?;
        d.set_item("constant", c.constant)?;
        d.set_item("suggested_cast", c.suggested_cast)?;
        columns.append(d)?;
    }
    let out = PyDict::new(py);
    out.set_item("total_rows", report.total_rows)?;
    out.set_item("columns", columns)?;
    Ok(out.into_any().unbind())
}
