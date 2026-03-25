#![deny(clippy::unwrap_used)]

//! Connector implementations for Bundlebase data sources.
//!
//! Each connector discovers and fetches data from a specific source type.

pub mod plugin;

#[cfg(test)]
pub(crate) mod test_utils;

// Re-export the Connector trait and associated types from common
pub use bundlebase_common::connector::*;
