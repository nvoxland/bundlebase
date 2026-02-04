use ::bundlebase::bundle_config::{ConfigKey, PassedBundleConfig, Scope};
use ::bundlebase::BundleConfig;
use pyo3::prelude::*;
use pyo3::types::PyDict;

#[pyclass(name = "BundleConfig")]
#[derive(Clone)]
pub struct PyBundleConfig {
    pub(crate) inner: PassedBundleConfig,
}

pub(crate) fn validate_config_key(key: &str) -> PyResult<()> {
    let specs = ::bundlebase::all_config_specs();
    if !ConfigKey::is_key_valid(key, &specs) {
        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            format!(
                "Unknown config key '{}'. Valid keys: {}",
                key,
                specs.iter().map(|s| s.key).collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    Ok(())
}

pub(crate) fn validate_config_key_scoped(key: &str, scope: &Scope) -> PyResult<()> {
    let specs = ::bundlebase::all_config_specs();
    ConfigKey::validate_key_scoped(key, scope, &specs).map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
    })
}

#[pymethods]
impl PyBundleConfig {
    #[pyo3(signature = (key, value, scope="/"))]
    fn set(&mut self, key: String, value: String, scope: &str) -> PyResult<()> {
        validate_config_key(&key)?;
        let scope = Scope::from_path(scope)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        self.inner.set(&key, &value, &scope);
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
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut config = PassedBundleConfig::new();

        for (key, value) in dict.iter() {
            let key_str: String = key.extract()?;

            // Check if value is a nested dict (scope-specific config)
            if let Ok(nested_dict) = value.downcast::<PyDict>() {
                // Scope-specific override
                let scope = Scope::from_path(&key_str)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                for (nested_key, nested_value) in nested_dict.iter() {
                    let nested_key_str: String = nested_key.extract()?;
                    let nested_value_str: String = nested_value.extract()?;
                    validate_config_key_scoped(&nested_key_str, &scope)?;
                    config.set(&nested_key_str, &nested_value_str, &scope);
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
