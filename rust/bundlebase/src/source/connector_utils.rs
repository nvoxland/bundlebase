//! Shared utilities for connector implementations.
//!
//! Re-exports common functions from `shared_utils` so that existing
//! `use crate::source::connector_utils::*` imports continue to work.

// Re-export everything from shared_utils that was previously defined here.
pub(crate) use super::shared_utils::{
    filename_from_url, get_patterns, matches_patterns, parse_patterns, read_http_version,
    record_batch_stream_to_parquet, require_arg, require_url, should_copy, stream_from_temp_file,
    validate_copy_arg, GuardedStream,
};
