use crate::bundle::operation::AnyOperation;
use crate::bundle::Bundle;
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

/// Context provided to commands during execution.
///
/// This provides a controlled interface for commands to interact with the
/// BundleBuilder without exposing its internals directly.
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

    /// Get a reference to the builder (for methods that need full builder access)
    pub fn builder(&self) -> &BundleBuilder {
        self.builder
    }

    /// Get a mutable reference to the builder (for methods that need full builder access)
    pub fn builder_mut(&mut self) -> &mut BundleBuilder {
        self.builder
    }

    /// Rebuild indexes after changes
    pub async fn reindex_internal(&mut self) -> Result<(), BundlebaseError> {
        self.builder.reindex_internal().await
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
    /// Execute the command using the provided context
    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError>;

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
    /// * `Ok(())` - Command executed successfully
    /// * `Err(BundlebaseError)` - Execution failed
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cmd = BundleCommand::Attach(AttachCommand::new("data.parquet", None));
    /// cmd.execute(&mut builder).await?;
    /// ```
    pub async fn execute(self, builder: &mut BundleBuilder) -> Result<(), BundlebaseError> {
        match self {
            // Standard commands delegate to execute_command
            BundleCommand::Attach(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::DetachBlock(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::Filter(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::DropColumn(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::RenameColumn(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::RenameView(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::Select(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::Join(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::CreateFunction(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::CreateIndex(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::DropIndex(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::DropView(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::DropJoin(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::RenameJoin(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::RebuildIndex(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::Reindex(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::ReplaceBlock(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::SetName(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::SetDescription(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::SetConfig(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::CreateSource(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::Fetch(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }
            BundleCommand::FetchAll(cmd) => {
                builder.execute_command(cmd).await?;
                Ok(())
            }

            // Special commands bypass execute_command
            BundleCommand::Commit(cmd) => {
                builder.commit(&cmd.message).await?;
                Ok(())
            }
            BundleCommand::Reset(_) => {
                builder.reset().await?;
                Ok(())
            }
            BundleCommand::Undo(_) => {
                builder.undo().await?;
                Ok(())
            }
        }
    }

    /// Add parameters to this command for parameterized queries.
    ///
    /// This method is used to bind parameters ($1, $2, etc.) in SQL statements if applicable.
    ///
    /// # Arguments
    ///
    /// * `params` - Vector of ScalarValue parameters
    ///
    /// # Returns
    ///
    /// * `Self` - The command with parameters added
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cmd = BundleCommand::Filter(FilterCommand::new("salary > $1", vec![]));
    /// let cmd_with_params = cmd.with_params(vec![
    ///     ScalarValue::Float64(Some(50000.0))
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
            other => other, // Other commands don't support parameters
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
