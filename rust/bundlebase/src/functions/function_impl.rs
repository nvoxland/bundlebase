use crate::functions::function_registry::FunctionSignature;
use crate::{BundlebaseError, DataGenerator};
use std::fmt::Debug;
use std::sync::Arc;

pub trait FunctionImpl: Debug + Sync + Send {
    fn execute(
        &self,
        sig: Arc<FunctionSignature>,
    ) -> Result<Arc<dyn DataGenerator>, BundlebaseError>;

    fn version(&self) -> String;
}
