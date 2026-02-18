//! User-defined functions for DataFusion queries
//!
//! This module provides custom scalar and aggregate functions that extend
//! DataFusion's SQL capabilities for Bundlebase.

mod bundle_info;
pub(crate) mod search_table_fn;

pub use bundle_info::VersionUdf;
pub use search_table_fn::SearchTableFunction;
