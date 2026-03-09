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
