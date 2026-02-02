use ::bundlebase::bundle_config::{PassedBundleConfig, Scope};
use pyo3::prelude::*;
use pyo3::types::PyDict;

#[pyclass(name = "BundleConfig")]
#[derive(Clone)]
pub struct PyBundleConfig {
    pub(crate) inner: PassedBundleConfig,
}

#[pymethods]
impl PyBundleConfig {
    #[pyo3(signature = (key, value, scope="/"))]
    fn set(&mut self, key: String, value: String, scope: &str) {
        let scope = Scope::from_url(scope);
        self.inner.set(&key, &value, &scope);
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
pub fn config_from_python(obj: &Bound<PyAny>) -> PyResult<PassedBundleConfig> {
    // config must be a dict
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut config = PassedBundleConfig::new();

        for (key, value) in dict.iter() {
            let key_str: String = key.extract()?;

            // Check if value is a nested dict (URL-specific config)
            if let Ok(nested_dict) = value.downcast::<PyDict>() {
                // URL-specific override
                let scope = Scope::from_url(&key_str);
                for (nested_key, nested_value) in nested_dict.iter() {
                    let nested_key_str: String = nested_key.extract()?;
                    let nested_value_str: String = nested_value.extract()?;
                    config.set(&nested_key_str, &nested_value_str, &scope);
                }
            } else {
                let value_str: String = value.extract()?;
                // Store all string values as global defaults.
                config.set(&key_str, &value_str, &Scope::global());
            }
        }

        return Ok(config);
    }

    Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
        "config must be a dict",
    ))
}
