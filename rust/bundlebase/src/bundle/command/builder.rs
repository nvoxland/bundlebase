//! Builder command trait and context.
//!
//! This module provides the `BundleBuilderCommand` trait for commands that mutate
//! a `BundleBuilder` and the `BuilderCommandContext` for command execution.

use crate::bundle::operation::AnyOperation;
use crate::bundle::{Bundle, BundleBuilder};
use crate::BundlebaseError;
use async_trait::async_trait;

// Re-export builder command implementations
mod attach;
mod commit;
mod create_index;
mod create_source;
mod detach_block;
mod drop_column;
mod drop_index;
mod drop_join;
mod drop_view;
mod fetch;
mod filter;
mod join;
mod rebuild_index;
mod reindex;
mod rename_column;
mod rename_join;
mod rename_view;
mod replace_block;
mod reset;
mod set_config;
mod set_description;
mod set_name;
mod undo;
mod verify_data;

pub use attach::AttachCommand;
pub use commit::CommitCommand;
pub use create_index::CreateIndexCommand;
pub use create_source::CreateSourceCommand;
pub use detach_block::DetachBlockCommand;
pub use drop_column::DropColumnCommand;
pub use drop_index::DropIndexCommand;
pub use drop_join::DropJoinCommand;
pub use drop_view::DropViewCommand;
pub use fetch::{FetchAllCommand, FetchCommand};
pub use filter::FilterCommand;
pub use join::JoinCommand;
pub use rebuild_index::RebuildIndexCommand;
pub use reindex::ReindexCommand;
pub use rename_column::RenameColumnCommand;
pub use rename_join::RenameJoinCommand;
pub use rename_view::RenameViewCommand;
pub use replace_block::ReplaceBlockCommand;
pub use reset::ResetCommand;
pub use set_config::SetConfigCommand;
pub use set_description::SetDescriptionCommand;
pub use set_name::SetNameCommand;
pub use undo::UndoCommand;
pub use verify_data::VerifyDataCommand;

/// Context provided to builder commands during execution.
///
/// This provides a controlled interface for commands to interact with the
/// BundleBuilder without exposing its internals directly.
///
/// # Preferred Access Patterns
///
/// Commands should use specific methods when available:
/// - `bundle()` - For read-only access to bundle state
/// - `data_dir()` - For the data directory (instead of `builder().data_dir()`)
/// - `apply_operation()` - To apply operations
///
/// Only use `builder()` when required by operation `setup()` methods.
pub struct BuilderCommandContext<'a> {
    pub(crate) builder: &'a mut BundleBuilder,
}

impl<'a> BuilderCommandContext<'a> {
    /// Create a new BuilderCommandContext wrapping a BundleBuilder
    pub fn new(builder: &'a mut BundleBuilder) -> Self {
        Self { builder }
    }

    /// Apply an operation to the bundle
    pub async fn apply_operation(&mut self, op: AnyOperation) -> Result<(), BundlebaseError> {
        self.builder.apply_operation(op).await
    }

    /// Get a reference to the bundle
    pub fn bundle(&self) -> &Bundle {
        &self.builder.bundle
    }

    /// Get a mutable reference to the bundle
    pub fn bundle_mut(&mut self) -> &mut Bundle {
        &mut self.builder.bundle
    }

    /// Get the data directory for the bundle.
    ///
    /// Use this instead of `builder().data_dir()`.
    pub fn data_dir(&self) -> &dyn crate::io::IOReadWriteDir {
        self.builder.data_dir()
    }

    /// Get a reference to the builder.
    ///
    /// This should only be used when required by operation `setup()` methods.
    /// For other access, prefer specific methods like `bundle()` or `data_dir()`.
    pub fn builder(&self) -> &BundleBuilder {
        self.builder
    }

    /// Get a mutable reference to the builder.
    ///
    /// This should only be used when required by methods that need mutable builder access.
    /// For most command implementations, use `apply_operation()` instead.
    pub fn builder_mut(&mut self) -> &mut BundleBuilder {
        self.builder
    }

    /// Rebuild indexes after changes
    pub async fn reindex_internal(&mut self) -> Result<(), BundlebaseError> {
        self.builder.reindex_internal().await
    }

    /// Clear all uncommitted status changes
    pub fn clear_status(&mut self) {
        self.builder.status.clear();
    }

    /// Pop the last uncommitted change from status
    pub fn pop_status(&mut self) {
        self.builder.status.pop();
    }

    /// Get a reference to the bundle status
    pub fn status(&self) -> &crate::bundle::builder::BundleStatus {
        &self.builder.status
    }

    /// Reload the bundle from the last committed state
    pub async fn reload_bundle(&mut self) -> Result<(), BundlebaseError> {
        self.builder.reload_bundle().await
    }

    /// Get the uncommitted changes list for reapplying
    pub fn status_changes(&self) -> &Vec<crate::bundle::operation::BundleChange> {
        self.builder.status.changes()
    }
}

/// Trait for commands that mutate a BundleBuilder.
///
/// These commands require mutable access to a `BundleBuilder` and typically
/// apply operations that change the bundle's state.
///
/// # Required Methods
///
/// All commands must implement via `CommandParsing`:
/// - `rule()` - Returns the pest Rule that matches this command
/// - `from_statement(pair)` - Parses from a pest Pair that matched the rule
/// - `to_statement()` - Serializes back to command string (round-trip support)
#[async_trait]
pub trait BundleBuilderCommand: super::CommandParsing {
    /// The type returned by execute().
    ///
    /// Most commands return `()`. Commands that need to return values
    /// (like fetch returning results, or verify_data returning verification results)
    /// can specify a different type.
    type Output;

    /// Execute the command using the provided context
    async fn execute(
        self: Box<Self>,
        ctx: &mut BuilderCommandContext<'_>,
    ) -> Result<Self::Output, BundlebaseError>;
}
