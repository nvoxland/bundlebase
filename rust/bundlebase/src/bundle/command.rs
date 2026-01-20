//! Command system for bundlebase operations.
//!
//! This module provides the command pattern implementation for bundlebase operations.
//! Commands encapsulate operation logic and can be executed via SQL parsing or direct API calls.
//!
//! # Command Execution Paths
//!
//! Commands can be executed through different paths depending on their characteristics:
//!
//! ## 1. `execute_command()` - For tracked unit commands
//!
//! Used for commands where `Output = ()` and changes should be tracked in status.
//! Wraps execution in `do_change()` for change tracking.
//!
//! ```ignore
//! builder.execute_command(AttachCommand::new("data.parquet", None)).await?;
//! ```
//!
//! ## 2. `run_command()` - For commands returning values
//!
//! Used for commands with `Output != ()`. Does not wrap in `do_change()`.
//! Operations within the command are still tracked at the operation level.
//!
//! ```ignore
//! let results: Vec<FetchResults> = builder.run_command(FetchCommand::new(None)).await?;
//! let verification = builder.run_command(VerifyDataCommand::new(false)).await?;
//! ```
//!
//! ## 3. Direct builder methods - For complex operations
//!
//! Some operations like `commit()`, `create_view()` have dedicated builder methods
//! that may perform additional logic beyond a single command.
//!
//! # Adding New Commands
//!
//! When adding a new command:
//!
//! 1. Create command struct in a new file under `command/`
//! 2. Implement the `Command` trait with appropriate `Output` type
//! 3. Add `mod` + `pub use` in this file
//! 4. Add variant to `BundleCommand` enum
//! 5. Add match arm in `BundleCommand::execute()`
//! 6. (If parseable) Add grammar rule in `parser/grammar.pest`
//! 7. (If parseable) Add match arm in `parser.rs::try_parse_pest()`
//!
//! Use `execute_command()` path if `Output = ()` and changes should be tracked.
//! Use `run_command()` path if the command returns meaningful results.

use crate::bundle::operation::AnyOperation;
use crate::bundle::{Bundle, VerificationResults};
use crate::source::FetchResults;
use crate::{BundleBuilder, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::ScalarValue;

pub mod parser;

// Re-export Rule from parser for use by commands
pub use parser::Rule;

// Command struct modules
mod attach;
mod commit;
mod create_index;
mod create_function;
mod create_source;
mod create_view;
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
mod replace_block;
mod rename_column;
mod rename_join;
mod rename_view;
mod reset;
mod select;
mod set_config;
mod set_description;
mod set_name;
mod undo;
mod verify_data;

// Re-export command structs
pub use attach::AttachCommand;
pub use commit::CommitCommand;
pub use create_function::CreateFunctionCommand;
pub use create_index::CreateIndexCommand;
pub use create_source::CreateSourceCommand;
pub use create_view::CreateViewCommand;
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
pub use replace_block::ReplaceBlockCommand;
pub use rename_join::RenameJoinCommand;
pub use rename_view::RenameViewCommand;
pub use reset::ResetCommand;
pub use select::SelectCommand;
pub use set_config::SetConfigCommand;
pub use set_description::SetDescriptionCommand;
pub use set_name::SetNameCommand;
pub use undo::UndoCommand;
pub use verify_data::VerifyDataCommand;

/// Output from executing a BundleCommand.
///
/// Most commands return Unit, but some commands return specific results
/// that may be useful to callers.
#[derive(Debug)]
pub enum CommandOutput {
    /// Command completed with no specific output
    Unit,
    /// Verification results from VERIFY DATA
    Verification(VerificationResults),
    /// Fetch results from FETCH / FETCH ALL
    Fetch(Vec<FetchResults>),
}

impl CommandOutput {
    /// Returns true if this is a Unit output
    pub fn is_unit(&self) -> bool {
        matches!(self, CommandOutput::Unit)
    }

    /// Get verification results if this is a Verification output
    pub fn into_verification(self) -> Option<VerificationResults> {
        match self {
            CommandOutput::Verification(r) => Some(r),
            _ => None,
        }
    }

    /// Get fetch results if this is a Fetch output
    pub fn into_fetch(self) -> Option<Vec<FetchResults>> {
        match self {
            CommandOutput::Fetch(r) => Some(r),
            _ => None,
        }
    }
}

/// Context provided to commands during execution.
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
pub struct CommandContext<'a> {
    pub(crate) builder: &'a mut BundleBuilder,
}

impl<'a> CommandContext<'a> {
    /// Create a new CommandContext wrapping a BundleBuilder
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

/// Trait for self-contained commands that can be executed on a BundleBuilder.
///
/// Commands encapsulate all the logic needed to perform a specific operation
/// on a bundle, including parsing and serialization for round-trip support.
///
/// # Parsing Methods
///
/// Commands that can be parsed from text implement:
/// - `rule()` - Returns the pest Rule that matches this command
/// - `from_pest()` - Parses from a pest Pair that matched the rule
///
/// All commands implement:
/// - `to_statement()` - Serializes back to command string (round-trip support)
#[async_trait]
pub trait Command: Send + Sync {
    /// The type returned by execute().
    ///
    /// Most commands return `()`. Commands that need to return values
    /// (like fetch returning results, or verify_data returning verification results)
    /// can specify a different type.
    type Output;

    /// Execute the command using the provided context
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<Self::Output, BundlebaseError>;

    /// The pest rule that matches this command (if applicable).
    ///
    /// Returns `None` by default. Commands that can be parsed from pest grammar
    /// override this to return the appropriate Rule variant.
    fn rule() -> Option<Rule>
    where
        Self: Sized,
    {
        None
    }

    /// Parse from a pest Pair that matched `Self::rule()`.
    ///
    /// Returns an error by default. Commands that can be parsed from pest grammar
    /// override this to implement parsing logic.
    fn from_pest(_pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError>
    where
        Self: Sized,
    {
        Err("Parsing not implemented for this command".into())
    }

    /// Serialize this command back to a statement string.
    ///
    /// This is used for:
    /// - Round-trip testing (parse -> to_statement -> re-parse)
    /// - Logging and debugging
    /// - Command history display
    fn to_statement(&self) -> String;
}

/// Command that can be executed on a BundleBuilder.
///
/// This enum wraps command structs, providing a single source of truth for command parameters.
/// Each variant delegates to its wrapped command struct for execution.
///
/// # Examples
///
/// ```ignore
/// use bundlebase::bundle::{BundleCommand, AttachCommand};
///
/// let cmd = BundleCommand::Attach(AttachCommand::new("data.parquet", None));
/// cmd.execute(&mut builder).await?;
/// ```
#[derive(Debug, Clone)]
pub enum BundleCommand {
    /// Attach a data source
    Attach(AttachCommand),

    /// Detach a data block by location
    DetachBlock(DetachBlockCommand),

    /// Filter rows by a WHERE condition
    Filter(FilterCommand),

    /// Remove a column
    DropColumn(DropColumnCommand),

    /// Rename a column
    RenameColumn(RenameColumnCommand),

    /// Rename a view
    RenameView(RenameViewCommand),

    /// Execute a full SQL query
    Select(SelectCommand),

    /// Join with another data source
    Join(JoinCommand),

    /// Create a custom function
    CreateFunction(CreateFunctionCommand),

    /// Create an index on a column
    CreateIndex(CreateIndexCommand),

    /// Drop an index on a column
    DropIndex(DropIndexCommand),

    /// Drop a view
    DropView(DropViewCommand),

    /// Drop a join
    DropJoin(DropJoinCommand),

    /// Rename a join
    RenameJoin(RenameJoinCommand),

    /// Rebuild an index on a column
    RebuildIndex(RebuildIndexCommand),

    /// Rebuild all indexes
    Reindex(ReindexCommand),

    /// Replace a block's location
    ReplaceBlock(ReplaceBlockCommand),

    /// Set bundle name
    SetName(SetNameCommand),

    /// Set bundle description
    SetDescription(SetDescriptionCommand),

    /// Set a configuration value
    SetConfig(SetConfigCommand),

    /// Commit changes
    Commit(CommitCommand),

    /// Reset uncommitted changes
    Reset(ResetCommand),

    /// Undo last change
    Undo(UndoCommand),

    /// Create a data source for fetching files
    CreateSource(CreateSourceCommand),

    /// Fetch new files from sources for a pack
    Fetch(FetchCommand),

    /// Fetch new files from all defined sources
    FetchAll(FetchAllCommand),

    /// Verify data integrity
    VerifyData(VerifyDataCommand),
}

impl BundleCommand {
    /// Execute this command on a BundleBuilder.
    ///
    /// This method delegates to the wrapped command struct via `execute_command`.
    ///
    /// # Arguments
    ///
    /// * `builder` - Mutable reference to the BundleBuilder to execute the command on
    ///
    /// # Returns
    ///
    /// * `Ok(CommandOutput)` - Command executed successfully with optional output
    /// * `Err(BundlebaseError)` - Execution failed
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cmd = BundleCommand::Attach(AttachCommand::new("data.parquet", None));
    /// let output = cmd.execute(&mut builder).await?;
    /// ```
    pub async fn execute(self, builder: &mut BundleBuilder) -> Result<CommandOutput, BundlebaseError> {
        match self {
            // Standard commands delegate to execute_command
            BundleCommand::Attach(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DetachBlock(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Filter(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DropColumn(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::RenameColumn(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::RenameView(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Select(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Join(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::CreateFunction(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::CreateIndex(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DropIndex(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DropView(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DropJoin(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::RenameJoin(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::RebuildIndex(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Reindex(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::ReplaceBlock(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::SetName(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::SetDescription(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::SetConfig(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::CreateSource(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Fetch(cmd) => {
                let results = builder.run_tracked_command(cmd).await?;
                Ok(CommandOutput::Fetch(results))
            }
            BundleCommand::FetchAll(cmd) => {
                let results = builder.run_tracked_command(cmd).await?;
                Ok(CommandOutput::Fetch(results))
            }

            // Special commands bypass execute_command
            BundleCommand::Commit(cmd) => {
                builder.commit(&cmd.message).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Reset(_) => {
                builder.reset().await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Undo(_) => {
                builder.undo().await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::VerifyData(cmd) => {
                let results = builder.run_command(cmd).await?;
                Ok(CommandOutput::Verification(results))
            }
        }
    }

    /// Add parameters to this command for parameterized queries.
    ///
    /// This method is used to bind parameters ($1, $2, etc.) in SQL statements.
    ///
    /// # Supported Commands
    ///
    /// Only the following commands support parameters:
    /// - `Filter` - Parameters in WHERE clause expressions
    /// - `Select` - Parameters in full SQL queries
    ///
    /// For other commands, this method returns the command unchanged.
    ///
    /// # Arguments
    ///
    /// * `params` - Vector of ScalarValue parameters to bind
    ///
    /// # Returns
    ///
    /// * `Self` - The command with parameters added (or unchanged if unsupported)
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Filter with parameter
    /// let cmd = BundleCommand::Filter(FilterCommand::new("salary > $1", vec![]));
    /// let cmd_with_params = cmd.with_params(vec![
    ///     ScalarValue::Float64(Some(50000.0))
    /// ]);
    ///
    /// // Select with parameters
    /// let cmd = BundleCommand::Select(SelectCommand::new(
    ///     "SELECT * FROM bundle WHERE id = $1 AND name = $2",
    ///     vec![]
    /// ));
    /// let cmd_with_params = cmd.with_params(vec![
    ///     ScalarValue::Int64(Some(42)),
    ///     ScalarValue::Utf8(Some("test".to_string())),
    /// ]);
    /// ```
    pub fn with_params(self, params: Vec<ScalarValue>) -> Self {
        match self {
            BundleCommand::Filter(mut cmd) => {
                cmd.params = params;
                BundleCommand::Filter(cmd)
            }
            BundleCommand::Select(mut cmd) => {
                cmd.params = params;
                BundleCommand::Select(cmd)
            }
            // Other commands don't support parameters - return unchanged
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::ScalarValue;
    use std::collections::HashMap;

    #[test]
    fn test_with_params_filter() {
        let cmd = BundleCommand::Filter(FilterCommand::new("salary > $1", vec![]));

        let params = vec![ScalarValue::Float64(Some(50000.0))];
        let cmd_with_params = cmd.with_params(params.clone());

        match cmd_with_params {
            BundleCommand::Filter(cmd) => {
                assert_eq!(cmd.where_clause, "salary > $1");
                assert_eq!(cmd.params.len(), 1);
            }
            _ => panic!("Expected Filter variant"),
        }
    }

    #[test]
    fn test_with_params_select() {
        let cmd = BundleCommand::Select(SelectCommand::new(
            "SELECT * FROM bundle WHERE id = $1",
            vec![],
        ));

        let params = vec![ScalarValue::Int64(Some(42))];
        let cmd_with_params = cmd.with_params(params.clone());

        match cmd_with_params {
            BundleCommand::Select(cmd) => {
                assert_eq!(cmd.sql, "SELECT * FROM bundle WHERE id = $1");
                assert_eq!(cmd.params.len(), 1);
            }
            _ => panic!("Expected Select variant"),
        }
    }

    #[test]
    fn test_with_params_other_command() {
        // with_params should have no effect on commands that don't support parameters
        let cmd = BundleCommand::Attach(AttachCommand::new("data.parquet", None));

        let params = vec![ScalarValue::Int64(Some(42))];
        let cmd_with_params = cmd.with_params(params);

        match cmd_with_params {
            BundleCommand::Attach(cmd) => {
                assert_eq!(cmd.path, "data.parquet");
                assert_eq!(cmd.pack, None);
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_attach_to_pack_command() {
        let cmd = BundleCommand::Attach(AttachCommand::new(
            "more_users.parquet",
            Some("users".to_string()),
        ));

        match cmd {
            BundleCommand::Attach(cmd) => {
                assert_eq!(cmd.path, "more_users.parquet");
                assert_eq!(cmd.pack, Some("users".to_string()));
            }
            _ => panic!("Expected Attach variant"),
        }
    }

    #[test]
    fn test_create_source_command() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/data/".to_string());
        args.insert("patterns".to_string(), "**/*.parquet".to_string());

        let cmd = BundleCommand::CreateSource(CreateSourceCommand::new(
            "remote_dir",
            args.clone(),
            None,
        ));

        match cmd {
            BundleCommand::CreateSource(cmd) => {
                assert_eq!(cmd.function, "remote_dir");
                assert_eq!(cmd.args.get("url"), Some(&"s3://bucket/data/".to_string()));
                assert_eq!(
                    cmd.args.get("patterns"),
                    Some(&"**/*.parquet".to_string())
                );
                assert_eq!(cmd.pack, None);
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_create_source_with_pack_command() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "s3://bucket/users/".to_string());

        let cmd = BundleCommand::CreateSource(CreateSourceCommand::new(
            "remote_dir",
            args,
            Some("users".to_string()),
        ));

        match cmd {
            BundleCommand::CreateSource(cmd) => {
                assert_eq!(cmd.function, "remote_dir");
                assert_eq!(cmd.pack, Some("users".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_fetch_command() {
        let cmd = BundleCommand::Fetch(FetchCommand::new(Some("users".to_string())));

        match cmd {
            BundleCommand::Fetch(cmd) => {
                assert_eq!(cmd.pack, Some("users".to_string()));
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_fetch_all_command() {
        let cmd = BundleCommand::FetchAll(FetchAllCommand::new());

        match cmd {
            BundleCommand::FetchAll(_) => {}
            _ => panic!("Expected FetchAll variant"),
        }
    }
}
