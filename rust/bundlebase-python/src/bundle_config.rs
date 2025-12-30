use ::bundlebase::BundleConfig;
use pyo3::prelude::*;
use pyo3::types::PyDict;

#[pyclass(name = "BundleConfig")]
#[derive(Clone)]
pub struct PyBundleConfig {
    pub(crate) inner: BundleConfig,
}

#[pymethods]
impl PyBundleConfig {
    #[new]
    fn new() -> Self {
        Self {
            inner: BundleConfig::new(),
        }
    }

    #[pyo3(signature = (key, value, url_prefix=None))]
    fn set(&mut self, key: String, value: String, url_prefix: Option<String>) {
        self.inner.set(&key, &value, url_prefix.as_deref());
    }

    fn __repr__(&self) -> String {
        format!("BundleConfig({:?})", self.inner)
    }
}

impl PyBundleConfig {
    pub fn into_inner(self) -> BundleConfig {
        self.inner
    }
}

/// Convert Python dict or BundleConfig to Rust BundleConfig
pub fn config_from_python(obj: &Bound<PyAny>) -> PyResult<BundleConfig> {
    // If it's already a PyBundleConfig, extract inner
    if let Ok(py_config) = obj.extract::<PyRef<PyBundleConfig>>() {
        return Ok(py_config.inner.clone());
    }

    // If it's a dict, convert to BundleConfig
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut config = BundleConfig::new();

        for (key, value) in dict.iter() {
            let key_str: String = key.extract()?;

            // Check if value is a nested dict (URL-specific config)
            if let Ok(nested_dict) = value.downcast::<PyDict>() {
                // URL-specific override
                for (nested_key, nested_value) in nested_dict.iter() {
                    let nested_key_str: String = nested_key.extract()?;
                    let nested_value_str: String = nested_value.extract()?;
                    config.set(&nested_key_str, &nested_value_str, Some(&key_str));
                }
            } else {
                // Simple string value - default config
                let value_str: String = value.extract()?;
                config.set(&key_str, &value_str, None);
            }
        }

        return Ok(config);
    }

    Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
        "config must be BundleConfig or dict",
    ))
}
