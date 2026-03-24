//! UDF runtime behavior trait and implementations.
//!
//! Each `UdfRuntime` variant wraps a concrete struct implementing `UdfEntrypoint`,
//! centralizing per-runtime logic that was previously scattered across match statements.

mod entrypoint;
pub(crate) mod ipc_utils;
mod runtime;

pub use entrypoint::RuntimeType;
pub use runtime::UdfRuntime;
