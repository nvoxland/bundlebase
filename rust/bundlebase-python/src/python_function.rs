//! Python function bridge implementation.
//!
//! Implements `PythonFunctionBridge` to invoke Python scalar and aggregate functions
//! from Rust during DataFusion query execution.

use arrow::array::ArrayRef;
use arrow::pyarrow::{FromPyArrow, ToPyArrow};
use bundlebase::function::lib_bridge::ManifestEntry;
use bundlebase::function::python_bridge::PythonFunctionBridge;
use bundlebase::BundlebaseError;
use datafusion::scalar::ScalarValue;
use pyo3::prelude::*;
use pyo3::types::PyTuple;

/// PyO3-based bridge that calls Python functions in-process.
pub struct PyFunctionBridge;

impl PyFunctionBridge {
    /// Import a Python module and instantiate a class from it.
    fn instantiate_class<'py>(
        py: Python<'py>,
        module: &str,
        class_name: &str,
    ) -> Result<Bound<'py, PyAny>, BundlebaseError> {
        let py_module = py
            .import(module)
            .map_err(|e| format!("Failed to import Python module '{}': {}", module, e))?;
        let py_class = py_module
            .getattr(class_name)
            .map_err(|e| {
                format!(
                    "Failed to find class '{}' in module '{}': {}",
                    class_name, module, e
                )
            })?;
        let instance = py_class
            .call0()
            .map_err(|e| {
                format!(
                    "Failed to instantiate class '{}' from module '{}': {}",
                    class_name, module, e
                )
            })?;
        Ok(instance)
    }

    /// Convert a ScalarValue to a PyArrow scalar.
    fn scalar_to_pyarrow<'py>(
        py: Python<'py>,
        value: &ScalarValue,
    ) -> Result<Bound<'py, PyAny>, BundlebaseError> {
        // Convert ScalarValue to a 1-element array, then extract element 0 via PyArrow
        let array = value
            .to_array()
            .map_err(|e| format!("Failed to convert ScalarValue to array: {}", e))?;
        let py_array = array
            .to_data()
            .to_pyarrow(py)
            .map_err(|e| format!("Failed to convert array to PyArrow: {}", e))?;

        // Get element [0] as a PyArrow scalar
        let pa = py
            .import("pyarrow")
            .map_err(|e| format!("Failed to import pyarrow: {}", e))?;
        let scalar_fn = pa
            .getattr("scalar")
            .map_err(|e| format!("Failed to get pyarrow.scalar: {}", e))?;

        // Extract the Python value from index 0 of the array
        let idx = 0i64.into_pyobject(py)
            .map_err(|e| format!("Failed to create index: {}", e))?;
        let element = py_array
            .call_method1("__getitem__", (idx,))
            .map_err(|e| format!("Failed to get array element: {}", e))?;

        // element is already a PyArrow scalar when indexing into a PyArrow array
        let _ = scalar_fn; // pyarrow.scalar not needed, __getitem__ on ChunkedArray/Array returns scalar
        Ok(element)
    }

    /// Convert a PyArrow scalar back to a ScalarValue.
    fn pyarrow_to_scalar(result: &Bound<'_, PyAny>) -> Result<ScalarValue, BundlebaseError> {
        // Call .as_py() to get the Python native value, then reconstruct
        let as_py = result
            .call_method0("as_py")
            .map_err(|e| format!("Failed to call as_py() on result: {}", e))?;

        // Get the PyArrow type to determine what ScalarValue to create
        let pa_type = result
            .getattr("type")
            .map_err(|e| format!("Failed to get .type on scalar: {}", e))?;
        let type_str: String = pa_type
            .str()
            .map_err(|e| format!("Failed to stringify type: {}", e))?
            .to_string();

        match type_str.as_str() {
            "int64" => {
                let val: i64 = as_py
                    .extract()
                    .map_err(|e| format!("Failed to extract int64: {}", e))?;
                Ok(ScalarValue::Int64(Some(val)))
            }
            "int32" => {
                let val: i32 = as_py
                    .extract()
                    .map_err(|e| format!("Failed to extract int32: {}", e))?;
                Ok(ScalarValue::Int32(Some(val)))
            }
            "float64" | "double" => {
                let val: f64 = as_py
                    .extract()
                    .map_err(|e| format!("Failed to extract float64: {}", e))?;
                Ok(ScalarValue::Float64(Some(val)))
            }
            "float32" | "float" => {
                let val: f32 = as_py
                    .extract()
                    .map_err(|e| format!("Failed to extract float32: {}", e))?;
                Ok(ScalarValue::Float32(Some(val)))
            }
            "string" | "utf8" => {
                let val: String = as_py
                    .extract()
                    .map_err(|e| format!("Failed to extract utf8: {}", e))?;
                Ok(ScalarValue::Utf8(Some(val)))
            }
            "bool" | "boolean" => {
                let val: bool = as_py
                    .extract()
                    .map_err(|e| format!("Failed to extract bool: {}", e))?;
                Ok(ScalarValue::Boolean(Some(val)))
            }
            s if s.starts_with("timestamp") => {
                // PyArrow timestamp as_py() returns a datetime.datetime
                // Extract as microseconds since epoch
                let py = as_py.py();
                let datetime_mod = py
                    .import("datetime")
                    .map_err(|e| format!("Failed to import datetime module: {}", e))?;
                let epoch = datetime_mod
                    .getattr("datetime")
                    .and_then(|dt| dt.call_method1("fromisoformat", ("1970-01-01T00:00:00+00:00",)))
                    .map_err(|e| format!("Failed to create epoch datetime: {}", e))?;
                // If the value is timezone-naive, use a naive epoch
                let has_tz = as_py
                    .getattr("tzinfo")
                    .map(|tz| !tz.is_none())
                    .unwrap_or(false);
                let epoch = if !has_tz {
                    datetime_mod
                        .getattr("datetime")
                        .and_then(|dt| dt.call_method1("fromisoformat", ("1970-01-01T00:00:00",)))
                        .map_err(|e| format!("Failed to create naive epoch: {}", e))?
                } else {
                    epoch
                };
                let delta = as_py
                    .call_method1("__sub__", (&epoch,))
                    .map_err(|e| format!("Failed to compute timedelta: {}", e))?;
                let total_seconds: f64 = delta
                    .call_method0("total_seconds")
                    .and_then(|v| v.extract())
                    .map_err(|e| format!("Failed to extract total_seconds: {}", e))?;
                let micros = (total_seconds * 1_000_000.0) as i64;
                Ok(ScalarValue::TimestampMicrosecond(Some(micros), None))
            }
            "date32" => {
                // PyArrow date32 as_py() returns a datetime.date
                // Convert to days since epoch
                let py = as_py.py();
                let datetime_mod = py
                    .import("datetime")
                    .map_err(|e| format!("Failed to import datetime module: {}", e))?;
                let epoch_date = datetime_mod
                    .getattr("date")
                    .and_then(|d| d.call1((1970, 1, 1)))
                    .map_err(|e| format!("Failed to create epoch date: {}", e))?;
                let delta = as_py
                    .call_method1("__sub__", (&epoch_date,))
                    .map_err(|e| format!("Failed to compute date delta: {}", e))?;
                let days: i32 = delta
                    .getattr("days")
                    .and_then(|v| v.extract())
                    .map_err(|e| format!("Failed to extract days: {}", e))?;
                Ok(ScalarValue::Date32(Some(days)))
            }
            "date64" => {
                // PyArrow date64 as_py() returns a datetime.date
                // Convert to milliseconds since epoch
                let py = as_py.py();
                let datetime_mod = py
                    .import("datetime")
                    .map_err(|e| format!("Failed to import datetime module: {}", e))?;
                let epoch_date = datetime_mod
                    .getattr("date")
                    .and_then(|d| d.call1((1970, 1, 1)))
                    .map_err(|e| format!("Failed to create epoch date: {}", e))?;
                let delta = as_py
                    .call_method1("__sub__", (&epoch_date,))
                    .map_err(|e| format!("Failed to compute date delta: {}", e))?;
                let days: i64 = delta
                    .getattr("days")
                    .and_then(|v| v.extract())
                    .map_err(|e| format!("Failed to extract days: {}", e))?;
                let millis = days * 86_400_000;
                Ok(ScalarValue::Date64(Some(millis)))
            }
            s if s.starts_with("decimal128") => {
                // PyArrow decimal128 as_py() returns a decimal.Decimal
                // Parse precision and scale from type string like "decimal128(38, 10)"
                let inner = s.trim_start_matches("decimal128(").trim_end_matches(')');
                let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
                let precision: u8 = parts.first()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(38);
                let scale: i8 = parts.get(1)
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0);
                // Multiply the decimal by 10^scale to get the unscaled integer,
                // then extract as an i128
                let py = as_py.py();
                let decimal_mod = py
                    .import("decimal")
                    .map_err(|e| format!("Failed to import decimal module: {}", e))?;
                let decimal_cls = decimal_mod
                    .getattr("Decimal")
                    .map_err(|e| format!("Failed to get Decimal class: {}", e))?;
                let multiplier = decimal_cls
                    .call1((10i64.pow(scale.unsigned_abs() as u32),))
                    .map_err(|e| format!("Failed to create decimal multiplier: {}", e))?;
                let unscaled = if scale >= 0 {
                    as_py.call_method1("__mul__", (&multiplier,))
                } else {
                    as_py.call_method1("__truediv__", (&multiplier,))
                }.map_err(|e| format!("Failed to compute unscaled decimal: {}", e))?;
                let unscaled_int = unscaled
                    .call_method0("__int__")
                    .map_err(|e| format!("Failed to convert decimal to int: {}", e))?;
                let val: i128 = unscaled_int
                    .extract()
                    .map_err(|e| format!("Failed to extract decimal i128: {}", e))?;
                Ok(ScalarValue::Decimal128(Some(val), precision, scale))
            }
            "binary" => {
                let val: Vec<u8> = as_py
                    .extract()
                    .map_err(|e| format!("Failed to extract binary: {}", e))?;
                Ok(ScalarValue::Binary(Some(val)))
            }
            "large_string" | "large_utf8" => {
                let val: String = as_py
                    .extract()
                    .map_err(|e| format!("Failed to extract large_utf8: {}", e))?;
                Ok(ScalarValue::LargeUtf8(Some(val)))
            }
            "large_binary" => {
                let val: Vec<u8> = as_py
                    .extract()
                    .map_err(|e| format!("Failed to extract large_binary: {}", e))?;
                Ok(ScalarValue::LargeBinary(Some(val)))
            }
            other => Err(format!(
                "Unsupported PyArrow scalar type '{}' in aggregate function result. \
                 Supported types: int32, int64, float32/float, float64/double, \
                 string/utf8/large_string/large_utf8, bool/boolean, \
                 timestamp, date32, date64, decimal128, binary, large_binary",
                other
            )
            .into()),
        }
    }
}

impl PythonFunctionBridge for PyFunctionBridge {
    fn invoke(
        &self,
        module: &str,
        function: &str,
        args: &[ArrayRef],
        _num_rows: usize,
    ) -> Result<ArrayRef, BundlebaseError> {
        let module = module.to_string();
        let function = function.to_string();
        let args = args.to_vec();

        Python::attach(|py| {
            let py_module = py
                .import(module.as_str())
                .map_err(|e| format!("Failed to import Python module '{}': {}", module, e))?;
            let py_func = py_module
                .getattr(function.as_str())
                .map_err(|e| {
                    format!(
                        "Failed to find function '{}' in module '{}': {}",
                        function, module, e
                    )
                })?;

            let py_args: Vec<Bound<'_, PyAny>> = args
                .iter()
                .map(|arr| {
                    arr.to_data()
                        .to_pyarrow(py)
                        .map_err(|e| {
                            BundlebaseError::from(format!(
                                "Failed to convert Arrow array to PyArrow: {}",
                                e
                            ))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;

            let py_tuple = PyTuple::new(py, &py_args)
                .map_err(|e| format!("Failed to create Python args tuple: {}", e))?;
            let result = py_func
                .call1(py_tuple)
                .map_err(|e| format!("Python function '{}:{}' raised an error: {}", module, function, e))?;

            let result_data = arrow::array::ArrayData::from_pyarrow_bound(&result)
                .map_err(|e| {
                    format!(
                        "Failed to convert Python function result to Arrow: {}. \
                         The function must return a PyArrow Array.",
                        e
                    )
                })?;
            let result_array: ArrayRef = arrow::array::make_array(result_data);

            Ok(result_array)
        })
    }

    fn aggregate_create_state(
        &self,
        module: &str,
        class_name: &str,
    ) -> Result<ScalarValue, BundlebaseError> {
        let module = module.to_string();
        let class_name = class_name.to_string();

        Python::attach(|py| {
            let instance = Self::instantiate_class(py, &module, &class_name)?;
            let result = instance
                .call_method0("create_state")
                .map_err(|e| {
                    format!(
                        "Python aggregate '{}:{}' create_state() failed: {}",
                        module, class_name, e
                    )
                })?;
            Self::pyarrow_to_scalar(&result)
        })
    }

    fn aggregate_accumulate(
        &self,
        module: &str,
        class_name: &str,
        state: &ScalarValue,
        args: &[ArrayRef],
    ) -> Result<ScalarValue, BundlebaseError> {
        let module = module.to_string();
        let class_name = class_name.to_string();
        let state = state.clone();
        let args = args.to_vec();

        Python::attach(|py| {
            let instance = Self::instantiate_class(py, &module, &class_name)?;

            // Convert state to PyArrow scalar
            let py_state = Self::scalar_to_pyarrow(py, &state)?;

            // Convert args to PyArrow arrays
            let py_args: Vec<Bound<'_, PyAny>> = args
                .iter()
                .map(|arr| {
                    arr.to_data()
                        .to_pyarrow(py)
                        .map_err(|e| {
                            BundlebaseError::from(format!(
                                "Failed to convert Arrow array to PyArrow: {}",
                                e
                            ))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;

            // Call accumulate(state, *values)
            let mut all_args = vec![py_state];
            all_args.extend(py_args);
            let py_tuple = PyTuple::new(py, &all_args)
                .map_err(|e| format!("Failed to create args tuple: {}", e))?;
            let result = instance
                .call_method1("accumulate", py_tuple)
                .map_err(|e| {
                    format!(
                        "Python aggregate '{}:{}' accumulate() failed: {}",
                        module, class_name, e
                    )
                })?;
            Self::pyarrow_to_scalar(&result)
        })
    }

    fn aggregate_merge(
        &self,
        module: &str,
        class_name: &str,
        state1: &ScalarValue,
        state2: &ScalarValue,
    ) -> Result<ScalarValue, BundlebaseError> {
        let module = module.to_string();
        let class_name = class_name.to_string();
        let state1 = state1.clone();
        let state2 = state2.clone();

        Python::attach(|py| {
            let instance = Self::instantiate_class(py, &module, &class_name)?;
            let py_state1 = Self::scalar_to_pyarrow(py, &state1)?;
            let py_state2 = Self::scalar_to_pyarrow(py, &state2)?;

            let result = instance
                .call_method1("merge", (py_state1, py_state2))
                .map_err(|e| {
                    format!(
                        "Python aggregate '{}:{}' merge() failed: {}",
                        module, class_name, e
                    )
                })?;
            Self::pyarrow_to_scalar(&result)
        })
    }

    fn aggregate_evaluate(
        &self,
        module: &str,
        class_name: &str,
        state: &ScalarValue,
    ) -> Result<ScalarValue, BundlebaseError> {
        let module = module.to_string();
        let class_name = class_name.to_string();
        let state = state.clone();

        Python::attach(|py| {
            let instance = Self::instantiate_class(py, &module, &class_name)?;
            let py_state = Self::scalar_to_pyarrow(py, &state)?;

            let result = instance
                .call_method1("evaluate", (py_state,))
                .map_err(|e| {
                    format!(
                        "Python aggregate '{}:{}' evaluate() failed: {}",
                        module, class_name, e
                    )
                })?;
            Self::pyarrow_to_scalar(&result)
        })
    }

    fn get_function_metadata(
        &self,
        module: &str,
    ) -> Result<Option<Vec<ManifestEntry>>, BundlebaseError> {
        let module = module.to_string();

        Python::attach(|py| {
            let py_module = py
                .import(module.as_str())
                .map_err(|e| format!("Failed to import Python module '{}': {}", module, e))?;

            // Check if the module has a bundlebase_metadata function
            let metadata_fn = match py_module.getattr("bundlebase_metadata") {
                Ok(attr) => attr,
                Err(_) => return Ok(None),
            };

            // Call bundlebase_metadata()
            let result = metadata_fn.call0().map_err(|e| {
                format!(
                    "Failed to call bundlebase_metadata() in module '{}': {}",
                    module, e
                )
            })?;

            // Parse the returned dict: {"functions": [{"name": ..., "input_types": [...], "return_type": ..., "kind": ...}]}
            let json_str: String = py
                .import("json")
                .map_err(|e| format!("Failed to import json module: {}", e))?
                .call_method1("dumps", (result,))
                .map_err(|e| {
                    format!(
                        "Failed to serialize bundlebase_metadata() result from '{}': {}",
                        module, e
                    )
                })?
                .extract()
                .map_err(|e| format!("Failed to extract JSON string: {}", e))?;

            let manifest: bundlebase::function::lib_bridge::Manifest =
                serde_json::from_str(&json_str).map_err(|e| {
                    format!(
                        "Failed to parse bundlebase_metadata() from '{}': {}. JSON: {}",
                        module, e, json_str
                    )
                })?;

            Ok(Some(manifest.functions))
        })
    }
}
