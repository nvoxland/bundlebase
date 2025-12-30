use arrow::datatypes::SchemaRef;
use arrow::pyarrow::{FromPyArrow, ToPyArrow};
use arrow::record_batch::RecordBatch;
use ::bundlebase::BundlebaseError;
use ::bundlebase::DataGenerator;
use pyo3::prelude::PyDictMethods;
use pyo3::types::{PyDict, PyFunction, PyTuple};
use pyo3::{Py, Python};
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;

#[derive(Debug)]
pub struct PythonDataGenerator {
    py_fn: Arc<Py<PyFunction>>,
    output: SchemaRef,
}

impl PythonDataGenerator {
    pub fn new(py_fn: Arc<Py<PyFunction>>, output: SchemaRef) -> PythonDataGenerator {
        Self { py_fn, output }
    }
}

impl Display for PythonDataGenerator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Python Data Generator")
    }
}

impl DataGenerator for PythonDataGenerator {
    fn next(&self, page: usize) -> Result<Option<RecordBatch>, BundlebaseError> {
        Python::attach(|py| {
            let kwargs = PyDict::new(py);
            kwargs.set_item("page", page)?;
            kwargs.set_item("schema", self.output.as_ref().to_pyarrow(py)?)?;
            match self.py_fn.call(py, PyTuple::empty(py), Some(&kwargs)) {
                Ok(value) => {
                    if value.is_none(py) {
                        return Ok(None);
                    }
                    match RecordBatch::from_pyarrow_bound(&value.into_bound(py)) {
                        Ok(batch) => Ok(Some(batch)),
                        Err(e) => Err(Box::new(e)),
                    }
                }
                Err(e) => Err(Box::new(e)),
            }
        })
        .map_err(Into::into)
    }
}
