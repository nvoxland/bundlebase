//! Facade command implementations.
//!
//! This module contains command implementations for read-only commands
//! that work with `&dyn BundleFacade`.

// Facade command implementations
mod explain;

pub use explain::ExplainPlanCommand;
