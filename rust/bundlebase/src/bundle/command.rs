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
//! These typically compute values (like ExplainPlan).
//!
//! # Adding New Commands
//!
//! Adding a new command is simplified via the `register_commands!` macro. You need to:
//!
//! 1. Create command struct in `command/builder/<name>.rs` or `command/facade/<name>.rs`
//! 2. Implement `CommandParsing` trait (`rule()`, `from_statement()`, `to_statement()`)
//! 3. Implement `BundleBuilderCommand` or `BundleFacadeCommand` trait
//! 4. Add re-export in `command/builder/mod.rs` or `command/facade/mod.rs`
//! 5. **Add one line to the `register_commands!` macro invocation** (see below)
//! 6. (If parseable) Add grammar rule in `parser/grammar.pest`
//!
//! The macro generates:
//! - `BundleCommand` enum variant
//! - Match arm in `BundleCommand::execute()`
//! - Match arm in `parse_from_rule()` for parser.rs
//!
//! # Command Categories (for the macro)
//!
//! - `message`: Commands that return String (most common)
//! - `fetch`: Commands that return `Vec<FetchResults>`
//! - `verification`: Commands that return `VerificationResults`
//! - `custom`: Commands with special execution logic (Commit, ExplainPlan)

use crate::bundle::facade::BundleFacade;
use crate::source::FetchResults;
use crate::{BundleBuilder, BundlebaseError};
use arrow::datatypes::SchemaRef;
use async_trait::async_trait;

pub mod parser;
pub mod builder;
pub mod facade;
pub mod response;

// Re-export response types
pub use response::OutputShape;
pub use response::CommandResponse;

// Re-export Rule from parser for use by commands
pub use parser::Rule;

// Re-export builder command structs
pub use builder::{
    AttachCommand, CommitCommand, CreateIndexCommand, CreateSourceCommand, CreateViewCommand,
    DetachBlockCommand, DropColumnCommand, DropIndexCommand, DropJoinCommand,
    DropViewCommand, FetchAllCommand, FetchCommand, FilterCommand, JoinCommand, RebuildIndexCommand,
    ReindexCommand, RenameColumnCommand, RenameJoinCommand, RenameViewCommand, ReplaceBlockCommand,
    ResetCommand, SetConfigCommand, SetDescriptionCommand, SetNameCommand, UndoCommand,
    VerifyDataCommand,
};

// Re-export verification result types
pub use builder::{FileVerificationResult, VerificationResults};

// Re-export facade command structs
pub use facade::ExplainPlanCommand;

/// Commands that can be executed on a BundleFacade (read-only).
///
/// This enum contains only commands that do not require mutation of the bundle.
/// It's a subset of `BundleCommand` that can be executed on a read-only `Bundle`.
#[derive(Debug, Clone)]
pub enum FacadeCommand {
    /// Show query execution plan
    ExplainPlan(ExplainPlanCommand),
}

impl FacadeCommand {
    /// Execute this command on a BundleFacade.
    pub async fn execute(
        self,
        facade: &dyn BundleFacade,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        match self {
            FacadeCommand::ExplainPlan(_) => {
                let plan = facade.explain().await?;
                Ok(Box::new(plan) as Box<dyn CommandResponse>)
            }
        }
    }

    /// Returns the Arrow schema for this command's output.
    pub fn output_schema(&self) -> SchemaRef {
        match self {
            FacadeCommand::ExplainPlan(_) => String::schema(),
        }
    }

    /// Returns the expected output shape for display formatting.
    pub fn output_shape(&self) -> OutputShape {
        match self {
            FacadeCommand::ExplainPlan(_) => String::output_shape(),
        }
    }
}

impl BundleCommand {
    /// Try to convert this command to a FacadeCommand.
    ///
    /// Returns `Ok(FacadeCommand)` if this is a read-only command (ExplainPlan).
    /// Returns `Err` with a descriptive error message if this is a mutating command.
    pub fn into_facade_command(self) -> Result<FacadeCommand, BundlebaseError> {
        match self {
            BundleCommand::ExplainPlan(cmd) => Ok(FacadeCommand::ExplainPlan(cmd)),
            _ => {
                // Get the command name for the error message
                let cmd_name = match &self {
                    BundleCommand::Attach(_) => "ATTACH",
                    BundleCommand::DetachBlock(_) => "DETACH",
                    BundleCommand::Filter(_) => "FILTER",
                    BundleCommand::Join(_) => "JOIN",
                    BundleCommand::ReplaceBlock(_) => "REPLACE",
                    BundleCommand::DropColumn(_) => "ALTER TABLE DROP COLUMN",
                    BundleCommand::RenameColumn(_) => "ALTER TABLE RENAME COLUMN",
                    BundleCommand::CreateIndex(_) => "CREATE INDEX",
                    BundleCommand::DropIndex(_) => "DROP INDEX",
                    BundleCommand::RebuildIndex(_) => "REBUILD INDEX",
                    BundleCommand::Reindex(_) => "REINDEX",
                    BundleCommand::CreateView(_) => "CREATE VIEW",
                    BundleCommand::RenameView(_) => "RENAME VIEW",
                    BundleCommand::DropView(_) => "DROP VIEW",
                    BundleCommand::DropJoin(_) => "DROP JOIN",
                    BundleCommand::RenameJoin(_) => "RENAME JOIN",
                    BundleCommand::SetName(_) => "SET NAME",
                    BundleCommand::SetDescription(_) => "SET DESCRIPTION",
                    BundleCommand::SetConfig(_) => "SET CONFIG",
                    BundleCommand::CreateSource(_) => "CREATE SOURCE",
                    BundleCommand::Reset(_) => "RESET",
                    BundleCommand::Undo(_) => "UNDO",
                    BundleCommand::Fetch(_) => "FETCH",
                    BundleCommand::FetchAll(_) => "FETCH ALL",
                    BundleCommand::VerifyData(_) => "VERIFY DATA",
                    BundleCommand::Commit(_) => "COMMIT",
                    BundleCommand::ExplainPlan(_) => {
                        unreachable!("Already handled above")
                    }
                };
                Err(format!(
                    "Cannot execute '{}' on read-only bundle. Open with --read-only=false to modify.",
                    cmd_name
                ).into())
            }
        }
    }

    /// Returns true if this command can be executed on a read-only bundle.
    pub fn is_facade_command(&self) -> bool {
        matches!(self, BundleCommand::ExplainPlan(_))
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
    /// All command output types must implement `CommandResponse` for consistent
    /// handling across different interfaces. Most commands return `String`,
    /// while commands like fetch and verify_data return their specific result types.
    type Output: CommandResponse;

    /// Execute the command on the provided builder
    async fn execute(
        self: Box<Self>,
        builder: &BundleBuilder,
    ) -> Result<Self::Output, BundlebaseError>;
}

/// Trait for read-only commands that work with `BundleFacade`.
///
/// These commands do not require mutable access to the bundle and can work
/// with any type that implements `BundleFacade`. They typically compute
/// and return a value from the current state (like ExplainPlan).
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
    /// All command output types must implement `CommandResponse` for consistent
    /// handling across different interfaces.
    type Output: CommandResponse;

    /// Execute the command on the provided facade
    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<Self::Output, BundlebaseError>;
}

/// Macro to register all commands with their categories.
///
/// This macro generates:
/// - `BundleCommand` enum variants
/// - Match arms in `BundleCommand::execute()`
/// - `parse_from_rule()` function for centralized rule-to-command mapping
///
/// # Categories
///
/// - `message`: Commands using `execute_command()` returning String (boxed as `dyn CommandResponse`)
/// - `fetch_special`: Commands returning `Vec<FetchResults>` with special parsing (handled in parser.rs)
/// - `verification`: Commands returning `VerificationResults`
/// - `custom`: Commands with custom execution logic (handled manually in execute)
///
/// Note: `fetch_special` commands are NOT included in `parse_from_rule()` because they share
/// grammar rules (e.g., fetch_stmt -> Fetch or FetchAll). They must be handled specially in parser.rs.
macro_rules! register_commands {
    (
        // Commands that return MessageResponse::ok()
        message {
            $( $msg_variant:ident($msg_cmd:ty) => $msg_rule:path ),* $(,)?
        }
        // Commands that return Vec<FetchResults> but need special parsing (shared rules)
        fetch_special {
            $( $fetch_variant:ident($fetch_cmd:ty) ),* $(,)?
        }
        // Commands that return VerificationResults
        verification {
            $( $verify_variant:ident($verify_cmd:ty) => $verify_rule:path ),* $(,)?
        }
        // Commands with custom execution logic (execute body provided separately)
        custom {
            $( $custom_variant:ident($custom_cmd:ty) => $custom_rule:path ),* $(,)?
        }
    ) => {
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
            // Message commands
            $( $msg_variant($msg_cmd), )*
            // Fetch commands (special parsing)
            $( $fetch_variant($fetch_cmd), )*
            // Verification commands
            $( $verify_variant($verify_cmd), )*
            // Custom commands
            $( $custom_variant($custom_cmd), )*
        }

        impl BundleCommand {
            /// Execute this command on a BundleBuilder.
            ///
            /// This method delegates to the wrapped command struct via `execute_command`.
            /// All commands return types implementing `CommandResponse`.
            pub async fn execute(self, builder: &BundleBuilder) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
                match self {
                    // Message commands - return String boxed as CommandResponse
                    $(
                        BundleCommand::$msg_variant(cmd) => {
                            let result = builder.execute_command(cmd).await?;
                            Ok(Box::new(result) as Box<dyn CommandResponse>)
                        }
                    )*
                    // Fetch commands - return Vec<FetchResults> boxed
                    $(
                        BundleCommand::$fetch_variant(cmd) => {
                            let results = builder.execute_command(cmd).await?;
                            Ok(Box::new(results) as Box<dyn CommandResponse>)
                        }
                    )*
                    // Verification commands - return VerificationResults boxed
                    $(
                        BundleCommand::$verify_variant(cmd) => {
                            let results = builder.execute_command(cmd).await?;
                            Ok(Box::new(results) as Box<dyn CommandResponse>)
                        }
                    )*
                    // Custom commands - handled individually below
                    BundleCommand::Commit(cmd) => {
                        let result = builder.execute_command(cmd).await?;
                        Ok(Box::new(result) as Box<dyn CommandResponse>)
                    }
                    BundleCommand::ExplainPlan(_cmd) => {
                        let plan = builder.explain().await?;
                        Ok(Box::new(plan) as Box<dyn CommandResponse>)
                    }
                }
            }

            /// Returns the Arrow schema that this command will produce when executed.
            pub fn output_schema(&self) -> SchemaRef {
                match self {
                    // Fetch commands
                    $( BundleCommand::$fetch_variant(_) => Vec::<FetchResults>::schema(), )*
                    // Verification commands
                    $( BundleCommand::$verify_variant(_) => VerificationResults::schema(), )*
                    // ExplainPlan returns plan schema (String)
                    BundleCommand::ExplainPlan(_) => String::schema(),
                    // All other commands return message schema
                    _ => String::schema(),
                }
            }

            /// Returns the expected output shape for display formatting.
            pub fn output_shape(&self) -> OutputShape {
                match self {
                    // Fetch commands return table format
                    $( BundleCommand::$fetch_variant(_) => Vec::<FetchResults>::output_shape(), )*
                    // Verification commands return table format
                    $( BundleCommand::$verify_variant(_) => VerificationResults::output_shape(), )*
                    // ExplainPlan returns single value (plan text)
                    BundleCommand::ExplainPlan(_) => String::output_shape(),
                    // All other commands return single value (OK message)
                    _ => String::output_shape(),
                }
            }
        }

        /// Parse a command from a pest Rule and Pair.
        ///
        /// This function provides centralized rule-to-command mapping, ensuring
        /// that adding a command only requires updating the `register_commands!` macro.
        ///
        /// Note: Commands in `fetch_special` category are NOT handled here because they
        /// share grammar rules. Handle them in `parse_command()` directly.
        ///
        /// # Arguments
        ///
        /// * `rule` - The pest Rule that was matched
        /// * `pair` - The pest Pair containing the parsed content
        ///
        /// # Returns
        ///
        /// * `Some(BundleCommand)` - If the rule matches a registered command
        /// * `None` - If the rule is not a command rule (use special handling in parser)
        pub fn parse_from_rule(rule: Rule, pair: pest::iterators::Pair<Rule>) -> Result<Option<BundleCommand>, BundlebaseError> {
            match rule {
                // Message commands
                $( $msg_rule => Ok(Some(BundleCommand::$msg_variant(<$msg_cmd>::from_statement(pair)?))), )*
                // Note: fetch_special commands are handled in parser.rs, not here
                // Verification commands
                $( $verify_rule => Ok(Some(BundleCommand::$verify_variant(<$verify_cmd>::from_statement(pair)?))), )*
                // Custom commands
                $( $custom_rule => Ok(Some(BundleCommand::$custom_variant(<$custom_cmd>::from_statement(pair)?))), )*
                // Unknown rule - return None for special handling
                _ => Ok(None),
            }
        }
    };
}

// Register all commands using the macro
//
// NOTE: Commands in `fetch_special` share the fetch_stmt rule and are handled
// specially in parser.rs::parse_command() rather than through parse_from_rule().
register_commands! {
    message {
        // Data modification commands
        Attach(AttachCommand) => Rule::attach_stmt,
        DetachBlock(DetachBlockCommand) => Rule::detach_stmt,
        Filter(FilterCommand) => Rule::filter_stmt,
        Join(JoinCommand) => Rule::join_stmt,
        ReplaceBlock(ReplaceBlockCommand) => Rule::replace_stmt,

        // Schema commands
        DropColumn(DropColumnCommand) => Rule::drop_column_stmt,
        RenameColumn(RenameColumnCommand) => Rule::rename_column_stmt,
        CreateIndex(CreateIndexCommand) => Rule::create_index_stmt,
        DropIndex(DropIndexCommand) => Rule::drop_index_stmt,
        RebuildIndex(RebuildIndexCommand) => Rule::rebuild_index_stmt,
        Reindex(ReindexCommand) => Rule::reindex_stmt,

        // View commands
        CreateView(CreateViewCommand) => Rule::create_view_stmt,
        RenameView(RenameViewCommand) => Rule::rename_view_stmt,
        DropView(DropViewCommand) => Rule::drop_view_stmt,

        // Join management commands
        DropJoin(DropJoinCommand) => Rule::drop_join_stmt,
        RenameJoin(RenameJoinCommand) => Rule::rename_join_stmt,

        // Metadata commands
        SetName(SetNameCommand) => Rule::set_name_stmt,
        SetDescription(SetDescriptionCommand) => Rule::set_description_stmt,
        SetConfig(SetConfigCommand) => Rule::set_config_stmt,

        // Source commands
        CreateSource(CreateSourceCommand) => Rule::create_source_stmt,

        // Transaction commands
        Reset(ResetCommand) => Rule::reset_stmt,
        Undo(UndoCommand) => Rule::undo_stmt,
    }
    fetch_special {
        // These commands share Rule::fetch_stmt - handled in parser.rs
        Fetch(FetchCommand),
        FetchAll(FetchAllCommand),
    }
    verification {
        VerifyData(VerifyDataCommand) => Rule::verify_data_stmt,
    }
    custom {
        Commit(CommitCommand) => Rule::commit_stmt,
        ExplainPlan(ExplainPlanCommand) => Rule::explain_stmt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
