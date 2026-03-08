//! Python native source bridge.
//!
//! Implements `NativePythonBridge` to allow Python `Connector` objects
//! to be used as native (in-process) data sources via PyO3.

use arrow::pyarrow::FromPyArrow;
use arrow::record_batch::RecordBatch;
use ::bundlebase::source::native::NativePythonBridge;
use ::bundlebase::BundlebaseError;
use pyo3::prelude::*;
use pyo3::types::PyList;
use std::collections::HashMap;

/// PyO3-based bridge that calls Python `Connector` methods in-process.
pub struct PyNativeBridge;

impl PyNativeBridge {
    /// Parse a `python:module:Class` call string into (module, class_name).
    fn parse_python_call(call: &str) -> Result<(&str, &str), BundlebaseError> {
        let rest = call
            .strip_prefix("python:")
            .ok_or_else(|| BundlebaseError::from("Expected python: prefix"))?;
        let parts: Vec<&str> = rest.rsplitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(format!(
                "Invalid python call format '{}'. Expected 'python:module:Class'",
                call
            )
            .into());
        }
        // rsplitn reverses order
        let module = parts[1];
        let class_name = parts[0];
        if module.is_empty() || class_name.is_empty() {
            return Err(format!(
                "Invalid python call format '{}'. Module and class name must not be empty",
                call
            )
            .into());
        }
        Ok((module, class_name))
    }

    /// Import the Python class and instantiate it.
    fn instantiate_source<'py>(
        py: Python<'py>,
        module: &str,
        class_name: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let py_module = py.import(module)?;
        let py_class = py_module.getattr(class_name)?;
        py_class.call0()
    }

    /// Parse args JSON into a Python-friendly HashMap, extracting attached_locations.
    fn parse_args_json(
        json: &str,
    ) -> Result<(HashMap<String, String>, Vec<String>), BundlebaseError> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|e| format!("Failed to parse args JSON: {}", e))?;

        let attached: Vec<String> = value
            .get("attached_locations")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let mut args = HashMap::new();
        if let Some(obj) = value.as_object() {
            for (k, v) in obj {
                if k == "attached_locations" {
                    continue;
                }
                if let Some(s) = v.as_str() {
                    args.insert(k.clone(), s.to_string());
                }
            }
        }

        Ok((args, attached))
    }
}

impl NativePythonBridge for PyNativeBridge {
    fn discover(&self, call: &str, args_json: &str) -> Result<String, BundlebaseError> {
        let (module, class_name) = Self::parse_python_call(call)?;
        let (args, attached) = Self::parse_args_json(args_json)?;

        Python::attach(|py| {
            let source = Self::instantiate_source(py, module, class_name)
                .map_err(|e| format!("Failed to instantiate Python source: {}", e))?;

            // Build kwargs from args
            let kwargs = pyo3::types::PyDict::new(py);
            for (k, v) in &args {
                kwargs
                    .set_item(k, v)
                    .map_err(|e| format!("Failed to set kwarg: {}", e))?;
            }

            // Build attached_locations list
            let py_attached = PyList::new(py, &attached)
                .map_err(|e| format!("Failed to create attached list: {}", e))?;

            // Call discover(attached_locations, **kwargs)
            let result = source
                .call_method("discover", (py_attached,), Some(&kwargs))
                .map_err(|e| format!("Python discover() failed: {}", e))?;

            // Convert list of Location objects to JSON
            let locations = result
                .downcast::<PyList>()
                .map_err(|_| BundlebaseError::from("discover() must return a list"))?;

            let mut json_locations = Vec::new();
            for loc in locations.iter() {
                let location: String = loc
                    .getattr("location")
                    .map_err(|e| format!("Location missing 'location' attr: {}", e))?
                    .extract()
                    .map_err(|e| format!("Location.location is not a string: {}", e))?;
                let must_copy: bool = loc
                    .getattr("must_copy")
                    .and_then(|v| v.extract())
                    .unwrap_or(true);
                let format: String = loc
                    .getattr("format")
                    .and_then(|v| v.extract())
                    .unwrap_or_else(|_| "parquet".to_string());
                let version: String = loc
                    .getattr("version")
                    .and_then(|v| v.extract())
                    .unwrap_or_default();

                json_locations.push(serde_json::json!({
                    "location": location,
                    "must_copy": must_copy,
                    "format": format,
                    "version": version,
                }));
            }

            let response = serde_json::json!({ "locations": json_locations });
            serde_json::to_string(&response)
                .map_err(|e| format!("Failed to serialize discover response: {}", e).into())
        })
    }

    fn data(
        &self,
        call: &str,
        location_json: &str,
        args_json: &str,
    ) -> Result<Option<Vec<RecordBatch>>, BundlebaseError> {
        let (module, class_name) = Self::parse_python_call(call)?;

        // Parse location
        let loc_value: serde_json::Value = serde_json::from_str(location_json)
            .map_err(|e| format!("Failed to parse location JSON: {}", e))?;

        // Parse args (without attached_locations)
        let args_value: serde_json::Value = serde_json::from_str(args_json)
            .map_err(|e| format!("Failed to parse args JSON: {}", e))?;
        let mut args = HashMap::new();
        if let Some(obj) = args_value.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    args.insert(k.clone(), s.to_string());
                }
            }
        }

        Python::attach(|py| {
            let source = Self::instantiate_source(py, module, class_name)
                .map_err(|e| format!("Failed to instantiate Python source: {}", e))?;

            // Import SDK Location and build it
            let sdk_module = py
                .import("bundlebase_sdk")
                .map_err(|e| format!("Failed to import bundlebase_sdk: {}", e))?;
            let location_class = sdk_module
                .getattr("Location")
                .map_err(|e| format!("Failed to get Location class: {}", e))?;

            let location_str = loc_value
                .get("location")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let must_copy = loc_value
                .get("must_copy")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let format = loc_value
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("parquet");
            let version = loc_value
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let kwargs = pyo3::types::PyDict::new(py);
            kwargs.set_item("must_copy", must_copy).ok();
            kwargs.set_item("format", format).ok();
            kwargs.set_item("version", version).ok();

            let py_location = location_class
                .call((location_str,), Some(&kwargs))
                .map_err(|e| format!("Failed to create Location object: {}", e))?;

            // Build data kwargs
            let data_kwargs = pyo3::types::PyDict::new(py);
            for (k, v) in &args {
                data_kwargs.set_item(k, v).ok();
            }

            // Call data(location, **kwargs)
            let result = source
                .call_method("data", (&py_location,), Some(&data_kwargs))
                .map_err(|e| format!("Python data() failed: {}", e))?;

            if result.is_none() {
                return Ok(None);
            }

            // Use the SDK's normalize_to_batches to handle all data types
            let protocol_module = py
                .import("bundlebase_sdk._protocol")
                .map_err(|e| format!("Failed to import bundlebase_sdk._protocol: {}", e))?;
            let normalize_fn = protocol_module
                .getattr("normalize_to_batches")
                .map_err(|e| format!("Failed to get normalize_to_batches: {}", e))?;

            // Get optional schema() from the source for dict-to-Arrow conversion
            let py_schema = source
                .call_method0("schema")
                .ok()
                .filter(|s| !s.is_none());

            let normalize_kwargs = pyo3::types::PyDict::new(py);
            if let Some(schema) = py_schema {
                normalize_kwargs.set_item("schema", schema).ok();
            }

            let py_batches = normalize_fn
                .call((&result,), Some(&normalize_kwargs))
                .map_err(|e| format!("normalize_to_batches failed: {}", e))?;

            let py_batch_list = py_batches
                .downcast::<PyList>()
                .map_err(|_| BundlebaseError::from("normalize_to_batches must return a list"))?;

            let mut batches = Vec::new();
            for batch_obj in py_batch_list.iter() {
                let batch = RecordBatch::from_pyarrow_bound(&batch_obj)
                    .map_err(|e| format!("Failed to convert PyArrow batch to Arrow: {}", e))?;
                batches.push(batch);
            }

            if batches.is_empty() {
                Ok(None)
            } else {
                Ok(Some(batches))
            }
        })
    }

    fn stable_url(
        &self,
        call: &str,
        location_json: &str,
        args_json: &str,
    ) -> Result<Option<String>, BundlebaseError> {
        let (module, class_name) = Self::parse_python_call(call)?;

        let loc_value: serde_json::Value = serde_json::from_str(location_json)
            .map_err(|e| format!("Failed to parse location JSON: {}", e))?;
        let args_value: serde_json::Value = serde_json::from_str(args_json)
            .map_err(|e| format!("Failed to parse args JSON: {}", e))?;

        Python::attach(|py| {
            let source = Self::instantiate_source(py, module, class_name)
                .map_err(|e| format!("Failed to instantiate Python source: {}", e))?;

            // Check if the source has a stable_url method
            if source.getattr("stable_url").is_err() {
                return Ok(None);
            }

            // Build location and kwargs (same as data)
            let sdk_module = py
                .import("bundlebase_sdk")
                .map_err(|e| format!("Failed to import bundlebase_sdk: {}", e))?;
            let location_class = sdk_module
                .getattr("Location")
                .map_err(|e| format!("Failed to get Location class: {}", e))?;

            let location_str = loc_value
                .get("location")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let must_copy = loc_value
                .get("must_copy")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let format = loc_value
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("parquet");
            let version = loc_value
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let loc_kwargs = pyo3::types::PyDict::new(py);
            loc_kwargs.set_item("must_copy", must_copy).ok();
            loc_kwargs.set_item("format", format).ok();
            loc_kwargs.set_item("version", version).ok();

            let py_location = location_class
                .call((location_str,), Some(&loc_kwargs))
                .map_err(|e| format!("Failed to create Location object: {}", e))?;

            let mut args = HashMap::new();
            if let Some(obj) = args_value.as_object() {
                for (k, v) in obj {
                    if let Some(s) = v.as_str() {
                        args.insert(k.clone(), s.to_string());
                    }
                }
            }

            let data_kwargs = pyo3::types::PyDict::new(py);
            for (k, v) in &args {
                data_kwargs.set_item(k, v).ok();
            }

            let result = source
                .call_method("stable_url", (&py_location,), Some(&data_kwargs))
                .map_err(|e| format!("Python stable_url() failed: {}", e))?;

            if result.is_none() {
                return Ok(None);
            }

            // Extract URL from StableUrl object or string
            if let Ok(url_str) = result.extract::<String>() {
                return Ok(Some(url_str));
            }

            if let Ok(url) = result.getattr("url") {
                if let Ok(url_str) = url.extract::<String>() {
                    return Ok(Some(url_str));
                }
            }

            Ok(None)
        })
    }
}
