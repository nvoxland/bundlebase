//! Facade command trait.
//!
//! This module provides the `BundleFacadeCommand` trait for read-only commands
//! that work with `&dyn BundleFacade`.
//!
//! Note: SelectCommand is conceptually a facade command but currently implements
//! BundleBuilderCommand for backwards compatibility. It resides here because
//! it could work with a read-only BundleFacade to produce a new BundleBuilder.

use crate::bundle::facade::BundleFacade;
use crate::BundlebaseError;
use async_trait::async_trait;

// Re-export facade command implementations
mod select;

pub use select::SelectCommand;

/// Trait for read-only commands that work with `BundleFacade`.
///
/// These commands do not require mutable access to the bundle and can work
/// with any type that implements `BundleFacade`. They typically either:
/// - Return a new `BundleBuilder` (like `SelectCommand`)
/// - Compute and return a value from the current state
///
/// # Required Methods
///
/// All commands must implement via `CommandParsing`:
/// - `rule()` - Returns the pest Rule that matches this command
/// - `from_statement(pair)` - Parses from a pest Pair that matched the rule
/// - `to_statement()` - Serializes back to command string (round-trip support)
#[async_trait]
pub trait BundleFacadeCommand: super::CommandParsing {
    /// The type returned by execute().
    ///
    /// For `SelectCommand`, this is `BundleBuilder` (a new builder with the query).
    /// Future commands might return other types like `usize` for count operations.
    type Output;

    /// Execute the command on the provided facade
    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<Self::Output, BundlebaseError>;
}
