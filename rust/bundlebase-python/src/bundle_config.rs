use ::bundlebase::bundle_config::{validated_scope, PassedBundleConfig, Scope};
use pyo3::prelude::*;
use pyo3::types::PyDict;

#[pyclass(name = "BundleConfig", skip_from_py_object)]
#[derive(Clone)]
pub struct PyBundleConfig {
    pub(crate) inner: PassedBundleConfig,
}

/// Parse a scope string from Python into a Scope.
///
/// Handles URLs like "s3://bucket", names like "s3/bucket", and simple names like "s3".
/// Validates against registered scopes so unknown scopes are rejected early.
pub(crate) fn parse_scope(scope: &str) -> PyResult<Scope> {
    validated_scope(scope)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
}

#[pymethods]
impl PyBundleConfig {
    fn set(&mut self, scope: &str, key: String, value: String) -> PyResult<()> {
        let scope = parse_scope(scope)?;
        self.inner.set(&scope, &key, &value);
        Ok(())
    }

    fn __repr__(&self) -> String {
        format!("BundleConfig({:?})", self.inner)
    }
}

impl PyBundleConfig {
    pub fn into_inner(self) -> PassedBundleConfig {
        self.inner
    }
}

/// Convert Python dict to Rust PassedBundleConfig
///
/// All values must be nested under a scope path key (e.g., `{"s3://": {"region": "us-west-2"}}`).
/// Flat top-level string keys are rejected.
pub fn config_from_python(obj: &Bound<PyAny>) -> PyResult<PassedBundleConfig> {
    // config must be a dict
    if let Ok(dict) = obj.cast::<PyDict>() {
        let mut config = PassedBundleConfig::new();

        for (key, value) in dict.iter() {
            let key_str: String = key.extract()?;

            // Check if value is a nested dict (scope-specific config)
            if let Ok(nested_dict) = value.cast::<PyDict>() {
                // Scope-specific override
                let scope = parse_scope(&key_str)?;
                for (nested_key, nested_value) in nested_dict.iter() {
                    let nested_key_str: String = nested_key.extract()?;
                    let nested_value_str: String = nested_value.extract()?;
                    config.set(&scope, &nested_key_str, &nested_value_str);
                }
            } else {
                return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    format!(
                        "Config key '{}' must be nested under a scope path. Example: {{\"s3://\": {{\"{}\": \"value\"}}}}",
                        key_str, key_str
                    ),
                ));
            }
        }

        return Ok(config);
    }

    Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
        "config must be a dict",
    ))
}
