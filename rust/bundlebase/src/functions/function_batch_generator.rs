use crate::DataGenerator;
use arrow::array::RecordBatch;
use datafusion::physical_plan::memory::LazyBatchGenerator;
use parking_lot::RwLock;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

#[derive(Debug)]
pub struct FunctionBatchGenerator {
    page: usize,
    generator: Arc<dyn DataGenerator>,
}

impl FunctionBatchGenerator {
    pub fn new(generator: Arc<dyn DataGenerator>) -> FunctionBatchGenerator {
        Self { generator, page: 0 }
    }
}

impl Display for FunctionBatchGenerator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Function Batch Generator (page: {})", self.page)
    }
}

impl LazyBatchGenerator for FunctionBatchGenerator {
    fn generate_next_batch(&mut self) -> datafusion::common::Result<Option<RecordBatch>> {
        self.page += 1;
        self.generator
            .next(self.page - 1)
            .map_err(|e| datafusion::common::DataFusionError::External(Box::from(e.to_string())))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn reset_state(&self) -> Arc<RwLock<dyn LazyBatchGenerator>> {
        Arc::new(RwLock::new(FunctionBatchGenerator::new(self.generator.clone())))
    }
}
