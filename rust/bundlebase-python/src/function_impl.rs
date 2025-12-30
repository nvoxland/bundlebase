use crate::data_generator::PythonDataGenerator;
use ::bundlebase::functions::FunctionImpl;
use ::bundlebase::functions::FunctionSignature;
use ::bundlebase::{BundlebaseError, DataGenerator};
use pyo3::types::PyFunction;
use pyo3::Py;
use std::sync::Arc;

#[derive(Debug)]
pub struct PythonFunctionImpl {
    function: Arc<Py<PyFunction>>,
    version: String,
}

impl PythonFunctionImpl {
    pub fn new(function: Py<PyFunction>, version: String) -> Self {
        Self {
            version,
            function: Arc::new(function),
        }
    }
}

impl FunctionImpl for PythonFunctionImpl {
    fn execute(
        &self,
        sig: Arc<FunctionSignature>,
    ) -> Result<Arc<dyn DataGenerator>, BundlebaseError> {
        Ok(Arc::new(PythonDataGenerator::new(
            self.function.clone(),
            sig.output().clone(),
        )))
    }

    fn version(&self) -> String {
        self.version.clone()
    }
}
