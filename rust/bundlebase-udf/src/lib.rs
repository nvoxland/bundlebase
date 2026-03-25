pub mod bridge;
pub mod runtime;
pub mod function_entry;
pub mod connector_entry;

// Re-export key types at crate root
pub use runtime::{UdfRuntime, RuntimeType};
pub use function_entry::{FunctionEntry, FunctionKind, FunctionRegistry, parse_function_name, validate_kind_consistency};
pub use connector_entry::{ConnectorEntry, resolve_connector, parse_connector_name};
pub use bridge::ipc_bridge::SubprocessCache;
pub use bridge::ipc_bridge::new_subprocess_cache;
pub use bridge::manifest::{Manifest, ManifestEntry};
pub use bridge::scalar::ScalarFunction;
pub use bridge::aggregate::AggregateFunction;
pub use bridge::version_function::VersionFunction;
pub use bridge::python_bridge::{PythonFunctionBridge, register_python_function_bridge, get_python_function_bridge};
pub use bridge::ffi_bridge;

/// Parse a Python entrypoint string in `"module:symbol"` format.
pub fn parse_python_entrypoint(entrypoint: &str) -> datafusion::common::Result<(&str, &str)> {
    let parts: Vec<&str> = entrypoint.rsplitn(2, ':').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(datafusion::common::DataFusionError::Execution(format!(
            "Invalid Python entrypoint '{}'. Expected 'module:symbol' format.",
            entrypoint
        )));
    }
    Ok((parts[1], parts[0]))
}
