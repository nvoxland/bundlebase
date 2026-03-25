//! Self-describing response types with Arrow schema support.
//!
//! This module re-exports the `CommandResponse` trait and related types from
//! `bundlebase_common::command_response`, which is the canonical source of truth.

pub use bundlebase_common::command_response::*;
pub use bundlebase_common::impl_dyn_command_response;
