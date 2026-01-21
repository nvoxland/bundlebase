//! Builder command trait.
//!
//! This module provides the `BundleBuilderCommand` trait for commands that mutate
//! a `BundleBuilder`.

use crate::bundle::BundleBuilder;
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

    /// Execute the command on the provided builder
    async fn execute(
        self: Box<Self>,
        builder: &mut BundleBuilder,
    ) -> Result<Self::Output, BundlebaseError>;
}
