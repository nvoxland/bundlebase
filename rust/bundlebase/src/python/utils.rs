use datafusion::scalar::ScalarValue;
use pyo3::prelude::*;

/// Convert Python objects to DataFusion ScalarValue
///
/// Supported types:
/// - bool → ScalarValue::Boolean
/// - int (i64) → ScalarValue::Int64
/// - float → ScalarValue::Float64
/// - str → ScalarValue::Utf8
/// - None → ScalarValue::Null
///
/// Returns a descriptive error if an unsupported type is provided,
/// including the parameter index and the actual type name.
pub fn convert_py_params(params: Vec<Py<PyAny>>) -> PyResult<Vec<ScalarValue>> {
    Python::attach(|py| {
        params
            .iter()
            .enumerate()
            .map(|(idx, py_obj)| {
                let obj = py_obj.bind(py);
                // Try to convert to supported types
                if let Ok(bool_val) = obj.extract::<bool>() {
                    Ok(ScalarValue::Boolean(Some(bool_val)))
                } else if let Ok(int_val) = obj.extract::<i64>() {
                    Ok(ScalarValue::Int64(Some(int_val)))
                } else if let Ok(float_val) = obj.extract::<f64>() {
                    Ok(ScalarValue::Float64(Some(float_val)))
                } else if let Ok(str_val) = obj.extract::<String>() {
                    Ok(ScalarValue::Utf8(Some(str_val)))
                } else if obj.is_none() {
                    Ok(ScalarValue::Null)
                } else {
                    // Provide helpful error message with parameter index and type
                    let type_name = obj
                        .get_type()
                        .name()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|_| "unknown".to_string());
                    Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(format!(
                        "Parameter at index {}: Unsupported type '{}'. \
                             Supported types: bool, int, float, str, None",
                        idx, type_name
                    )))
                }
            })
            .collect()
    })
}
