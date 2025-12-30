use crate::functions::function_impl::FunctionImpl;
use crate::functions::function_registry::FunctionSignature;
use crate::{BundlebaseError, DataGenerator};
use arrow::array::RecordBatch;
use std::sync::Arc;

#[derive(Debug)]
pub struct StaticImpl {
    data: Arc<Vec<RecordBatch>>,
    version: String,
}

impl StaticImpl {
    pub fn new(data: Vec<RecordBatch>, version: String) -> Self {
        Self {
            data: Arc::new(data),
            version,
        }
    }
}

#[derive(Debug)]
struct StaticGenerator {
    data: Arc<Vec<RecordBatch>>,
}

impl FunctionImpl for StaticImpl {
    fn execute(
        &self,
        _: Arc<FunctionSignature>,
    ) -> Result<Arc<dyn DataGenerator>, BundlebaseError> {
        Ok(Arc::new(StaticGenerator {
            data: self.data.clone(),
        }))
    }

    fn version(&self) -> String {
        self.version.clone()
    }
}

impl DataGenerator for StaticGenerator {
    fn next(&self, page: usize) -> Result<Option<RecordBatch>, BundlebaseError> {
        if page >= self.data.len() {
            return Ok(None);
        }

        Ok(Some(self.data[page].clone()))
    }
}
