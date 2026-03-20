//! Bridge trait for invoking Python functions from Rust.
//!
//! The core crate defines the trait; `bundlebase-python` implements it
//! via PyO3 and registers it at module init time.

use arrow::array::ArrayRef;
use datafusion::scalar::ScalarValue;
use crate::function::manifest::ManifestEntry;
use crate::BundlebaseError;
use std::sync::{Arc, OnceLock};

/// Trait that the Python bindings implement to invoke functions.
pub trait PythonFunctionBridge: Send + Sync {
    /// Invoke a Python scalar function with Arrow array arguments.
    fn invoke(
        &self,
        module: &str,
        function: &str,
        args: &[ArrayRef],
        num_rows: usize,
    ) -> Result<ArrayRef, BundlebaseError>;

    /// Create initial accumulator state for an aggregate function.
    fn aggregate_create_state(
        &self,
        module: &str,
        class_name: &str,
    ) -> Result<ScalarValue, BundlebaseError>;

    /// Accumulate a batch of values into the aggregate state.
    fn aggregate_accumulate(
        &self,
        module: &str,
        class_name: &str,
        state: &ScalarValue,
        args: &[ArrayRef],
    ) -> Result<ScalarValue, BundlebaseError>;

    /// Merge two aggregate states (for parallel execution).
    fn aggregate_merge(
        &self,
        module: &str,
        class_name: &str,
        state1: &ScalarValue,
        state2: &ScalarValue,
    ) -> Result<ScalarValue, BundlebaseError>;

    /// Produce the final result from aggregate state.
    fn aggregate_evaluate(
        &self,
        module: &str,
        class_name: &str,
        state: &ScalarValue,
    ) -> Result<ScalarValue, BundlebaseError>;

    /// Get function metadata from a Python module's `bundlebase_metadata()` function.
    ///
    /// Returns `Ok(None)` if the module doesn't define `bundlebase_metadata`.
    fn get_function_metadata(
        &self,
        module: &str,
    ) -> Result<Option<Vec<ManifestEntry>>, BundlebaseError>;
}

/// Global bridge set by `bundlebase-python` at module init time.
static PYTHON_FUNCTION_BRIDGE: OnceLock<Arc<dyn PythonFunctionBridge>> = OnceLock::new();

/// Register the Python function bridge. Called once from `bundlebase-python` init.
pub fn register_python_function_bridge(bridge: Arc<dyn PythonFunctionBridge>) {
    let _ = PYTHON_FUNCTION_BRIDGE.set(bridge);
}

/// Get the registered Python function bridge.
pub fn get_python_function_bridge() -> Result<&'static Arc<dyn PythonFunctionBridge>, BundlebaseError> {
    PYTHON_FUNCTION_BRIDGE
        .get()
        .ok_or_else(|| "Python function bridge not initialized. Are you running from Python?".into())
}
