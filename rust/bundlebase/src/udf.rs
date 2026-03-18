//! UDF runtime behavior trait and implementations.
//!
//! Each `UdfRuntime` variant wraps a concrete struct implementing `UdfEntrypoint`,
//! centralizing per-runtime logic that was previously scattered across match statements.

mod entrypoint;
mod runtime;

pub use entrypoint::{RuntimeType, UdfEntrypoint};
pub use runtime::UdfRuntime;
pub use runtime::{PythonRuntime, FfiRuntime, IpcRuntime, JavaRuntime, DockerRuntime};
pub use crate::function::lib_bridge::{Manifest, ManifestEntry};
