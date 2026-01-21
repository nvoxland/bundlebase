//! Facade command implementations.
//!
//! This module contains command implementations for read-only commands
//! that work with `&dyn BundleFacade`.
//!
//! Note: SelectCommand is conceptually a facade command but currently implements
//! BundleBuilderCommand for backwards compatibility. It resides here because
//! it could work with a read-only BundleFacade to produce a new BundleBuilder.

// Facade command implementations
mod explain;
mod select;

pub use explain::ExplainPlanCommand;
pub use select::SelectCommand;
