//! Command system for bundlebase operations.
//!
//! This module provides the command pattern implementation for bundlebase operations.
//! Commands encapsulate operation logic and can be executed via SQL parsing or direct API calls.
//!
//! # Command Types
//!
//! Commands are divided into two categories based on their requirements:
//!
//! ## BundleBuilderCommand - Mutating Commands
//!
//! Commands that require `&mut BundleBuilder` because they modify state.
//! Most commands fall into this category (attach, filter, commit, etc.).
//!
//! ## BundleFacadeCommand - Read-Only Commands
//!
//! Commands that work with `&dyn BundleFacade` and don't need to mutate the source.
//! These typically return a new `BundleBuilder` or compute values.
//!
//! Currently only `SelectCommand` is a facade command - it returns a NEW builder
//! with the query applied rather than mutating the source.
//!
//! # Command Execution Paths
//!
//! Commands can be executed through different paths depending on their characteristics:
//!
//! ## 1. `execute_builder_command()` - For tracked unit commands
//!
//! Used for commands where `Output = ()` and changes should be tracked in status.
//! Wraps execution in `do_change()` for change tracking.
//!
//! ```ignore
//! builder.execute_builder_command(AttachCommand::new("data.parquet", None)).await?;
//! ```
//!
//! ## 2. `run_builder_command()` - For commands returning values
//!
//! Used for commands with `Output != ()`. Does not wrap in `do_change()`.
//! Operations within the command are still tracked at the operation level.
//!
//! ```ignore
//! let results: Vec<FetchResults> = builder.run_builder_command(FetchCommand::new(None)).await?;
//! let verification = builder.run_builder_command(VerifyDataCommand::new(false)).await?;
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
//! 1. Determine if it's a builder command (mutates state) or facade command (read-only)
//! 2. Create command struct in appropriate directory:
//!    - `command/builder/` for BundleBuilderCommand
//!    - `command/facade/` for BundleFacadeCommand
//! 3. Implement the `CommandParsing` trait for parsing/serialization
//! 4. Implement the appropriate command trait (`BundleBuilderCommand` or `BundleFacadeCommand`)
//! 5. Add variant to `BundleCommand` enum
//! 6. Add match arm in `BundleCommand::execute()`
//! 7. (If parseable) Add grammar rule in `parser/grammar.pest`
//! 8. (If parseable) Add match arm in `parser.rs::try_parse_pest()`
//!
//! Use `execute_builder_command()` path if `Output = ()` and changes should be tracked.
//! Use `run_builder_command()` path if the command returns meaningful results.

use crate::bundle::VerificationResults;
use crate::bundle::facade::BundleFacade;
use crate::source::FetchResults;
use crate::{BundleBuilder, BundlebaseError};
use async_trait::async_trait;
use datafusion::common::ScalarValue;

pub mod parser;
pub mod builder;
pub mod facade;

// Re-export Rule from parser for use by commands
pub use parser::Rule;

// Re-export builder command structs
pub use builder::{
    AttachCommand, CommitCommand, CreateIndexCommand, CreateSourceCommand, DetachBlockCommand,
    DropColumnCommand, DropIndexCommand, DropJoinCommand, DropViewCommand, FetchAllCommand,
    FetchCommand, FilterCommand, JoinCommand, RebuildIndexCommand, ReindexCommand,
    RenameColumnCommand, RenameJoinCommand, RenameViewCommand, ReplaceBlockCommand, ResetCommand,
    SetConfigCommand, SetDescriptionCommand, SetNameCommand, UndoCommand, VerifyDataCommand,
};

// Re-export facade command structs
pub use facade::SelectCommand;

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

/// Trait for command parsing and serialization.
///
/// This trait provides the common parsing/serialization methods that all commands
/// must implement, regardless of whether they are builder or facade commands.
pub trait CommandParsing: Send + Sync {
    /// The pest rule that matches this command.
    ///
    /// Every command must have an associated grammar rule for SQL parsing.
    fn rule() -> Rule
    where
        Self: Sized;

    /// Parse from a pest Pair that matched `Self::rule()`.
    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError>
    where
        Self: Sized;

    /// Serialize this command back to a statement string.
    ///
    /// This is used for:
    /// - Round-trip testing (parse -> to_statement -> re-parse)
    /// - Logging and debugging
    /// - Command history display
    fn to_statement(&self) -> String;
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
pub trait BundleBuilderCommand: CommandParsing {
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
pub trait BundleFacadeCommand: CommandParsing {
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
    // === Builder Commands (mutate state) ===

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

    /// Join with another data source
    Join(JoinCommand),

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

    // === Facade Commands (read-only, return new BundleBuilder) ===

    /// Execute a full SQL query (returns new BundleBuilder)
    Select(SelectCommand),
}

impl BundleCommand {
    /// Execute this command on a BundleBuilder.
    ///
    /// This method delegates to the wrapped command struct via `execute_builder_command`.
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
            // Builder commands delegate to execute_builder_command
            BundleCommand::Attach(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DetachBlock(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Filter(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DropColumn(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::RenameColumn(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::RenameView(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Select(cmd) => {
                // Select uses the BundleFacade's select() method which returns a new builder
                let new_builder = builder.select(&cmd.sql, cmd.params).await?;
                *builder = new_builder;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Join(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::CreateIndex(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DropIndex(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DropView(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::DropJoin(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::RenameJoin(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::RebuildIndex(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Reindex(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::ReplaceBlock(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::SetName(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::SetDescription(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::SetConfig(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::CreateSource(cmd) => {
                builder.execute_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Fetch(cmd) => {
                let results = builder.run_tracked_builder_command(cmd).await?;
                Ok(CommandOutput::Fetch(results))
            }
            BundleCommand::FetchAll(cmd) => {
                let results = builder.run_tracked_builder_command(cmd).await?;
                Ok(CommandOutput::Fetch(results))
            }

            // Special commands bypass execute_builder_command
            BundleCommand::Commit(cmd) => {
                builder.commit(&cmd.message).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Reset(cmd) => {
                builder.run_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::Undo(cmd) => {
                builder.run_builder_command(cmd).await?;
                Ok(CommandOutput::Unit)
            }
            BundleCommand::VerifyData(cmd) => {
                let results = builder.run_builder_command(cmd).await?;
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
