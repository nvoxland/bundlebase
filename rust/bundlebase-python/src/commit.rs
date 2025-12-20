use super::builder::PyChange;
use super::operation::PyOperation;
use ::bundlebase::bundle::BundleCommit;
use pyo3::prelude::*;

#[pyclass]
#[derive(Clone)]
pub struct PyCommit {
    inner: BundleCommit,
}

#[pymethods]
impl PyCommit {
    #[getter]
    fn author(&self) -> String {
        self.inner.author.clone()
    }

    #[getter]
    fn message(&self) -> String {
        self.inner.message.clone()
    }

    #[getter]
    fn timestamp(&self) -> String {
        self.inner.timestamp.clone()
    }

    #[getter]
    fn changes(&self) -> Vec<PyChange> {
        self.inner
            .changes
            .iter()
            .map(|change| PyChange::from_rust(change))
            .collect()
    }

    #[getter]
    fn operations(&self) -> Vec<PyOperation> {
        self.inner
            .operations()
            .iter()
            .map(|op_config| PyOperation::new(op_config.clone()))
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "PyCommit(author='{}', message='{}', timestamp='{}', changes={})",
            self.inner.author,
            self.inner.message,
            self.inner.timestamp,
            self.inner.changes.len()
        )
    }

    fn __str__(&self) -> String {
        format!(
            "{} - {} ({}): {} change(s)",
            self.inner.timestamp,
            self.inner.author,
            self.inner.message,
            self.inner.changes.len()
        )
    }
}

impl PyCommit {
    pub fn new(inner: BundleCommit) -> Self {
        PyCommit { inner }
    }
}
