pub mod bridge;
pub mod connector_entry;
pub mod function_entry;
pub mod runtime;

// Re-export key types at crate root
pub use bridge::aggregate::AggregateFunction;
pub use bridge::ffi_bridge;
pub use bridge::ipc_bridge::new_subprocess_cache;
pub use bridge::ipc_bridge::SubprocessCache;
pub use bridge::manifest::{Manifest, ManifestEntry};
pub use bridge::python_bridge::{
    get_python_function_bridge, register_python_function_bridge, PythonFunctionBridge,
};
pub use bridge::scalar::ScalarFunction;
pub use bridge::version_function::VersionFunction;
pub use connector_entry::{parse_connector_name, resolve_connector, ConnectorEntry};
pub use function_entry::{
    internal_function_name, parse_function_name, validate_kind_consistency, FunctionEntry,
    FunctionKind, FunctionRegistry,
};
pub use runtime::{RuntimeType, UdfRuntime};

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
