//! User-defined functions for DataFusion queries
//!
//! This module provides custom scalar and aggregate functions that extend
//! DataFusion's SQL capabilities for Bundlebase.

mod text_search;

pub use text_search::{extract_text_search_args, TextSearchUdf, TextSearchUdfState};
