//! Functions for DataFusion queries
//!
//! This module provides custom scalar and aggregate functions that extend
//! DataFusion's SQL capabilities for Bundlebase.

mod bundle_info;
pub mod ipc_bridge;
pub mod lib_bridge;
pub mod python_bridge;
pub mod scalar;
pub mod aggregate;

pub use bundle_info::VersionFunction;

/// Parse a Python logic string in `"module:symbol"` format.
///
/// Uses `rsplitn` so that dotted module paths (e.g. `pkg.sub.mod:func`) work
/// correctly — only the *last* colon is treated as the delimiter.
///
/// Returns `(module, symbol)` on success.
pub(crate) fn parse_python_logic(logic: &str) -> datafusion::common::Result<(&str, &str)> {
    let parts: Vec<&str> = logic.rsplitn(2, ':').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(datafusion::common::DataFusionError::Execution(format!(
            "Invalid Python logic '{}'. Expected 'module:symbol' format.",
            logic
        )));
    }
    // rsplitn reverses order
    Ok((parts[1], parts[0]))
}
