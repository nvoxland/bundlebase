#![deny(clippy::unwrap_used)]
//! Command system for bundlebase operations.
//!
//! This crate provides the command pattern implementation for bundlebase operations.
//! Commands encapsulate operation logic and can be executed via SQL parsing or direct API calls.
//!
//! # Command Types
//!
//! Commands are divided into two categories based on their requirements:
//!
//! ## BundleBuilderCommand - Mutating Commands
//!
//! Commands that require `&BundleBuilder` because they modify state.
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
//! 1. Create command struct in `builder/<name>.rs` or `facade/<name>.rs`
//! 2. Implement `CommandParsing` trait (`rule()`, `from_statement()`, `to_statement()`)
//! 3. Implement `BundleBuilderCommand` or `BundleFacadeCommand` trait
//! 4. Add re-export in `builder.rs` or `facade.rs`
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
//! - `facade`: Read-only commands using `BundleFacadeCommand` (ExplainPlan)

use bundlebase::BundleFacade;
use bundlebase::source::FetchResults;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;
use arrow::datatypes::SchemaRef;

pub mod parser;
pub mod builder;
pub mod facade;
pub mod response;
pub mod facade_ext;
pub mod builder_ext;
pub mod sql_utils;

// Re-export response types
pub use response::OutputShape;
pub use response::CommandResponse;
pub use bundlebase_common::impl_dyn_command_response;

// Re-export Rule from parser for use by commands
pub use parser::Rule;

// Re-export builder command structs
pub use builder::{
    AddColumnCommand, AlwaysDeleteCommand, AlwaysUpdateCommand, AttachCommand, CastColumnCommand, CommitCommand, CreateIndexCommand, CreateSourceCommand,
    DeleteCommand, DropAlwaysDeleteCommand, DropAlwaysUpdateCommand, DropCastColumnCommand, UpdateCommand, ImportJoinCommand, ImportConnectorCommand, ImportFunctionCommand, CreateViewCommand, DetachBlockCommand,
    DropColumnCommand, DropConnectorCommand, DropFunctionCommand, DropIndexCommand, DropJoinCommand,
    DropViewCommand, FetchAllCommand, FetchCommand, FilterCommand, JoinCommand,
    RebuildIndexCommand, ReindexCommand, RenameColumnCommand, RenameConnectorCommand,
    RenameFunctionCommand, RenameJoinCommand, RenameViewCommand,
    ReplaceBlockCommand, ResetCommand, SaveConfigCommand, SetDescriptionCommand, SetNameCommand,
    NormalizeColumnNamesCommand, UndoCommand, VerifyDataCommand, ExportHollowCommand,
};

// Re-export verification result types
pub use builder::{FileVerificationResult, VerificationResults};

// Re-export facade command structs
pub use facade::DescribeDataCommand;
pub use facade::ExportDataCommand;
pub use facade::TestConnectorCommand;
pub use facade::DescribeConnectorCommand;
pub use facade::DescribeFunctionCommand;
pub use facade::ImportTempConnectorCommand;
pub use facade::ImportTempFunctionCommand;
pub use facade::DropTempConnectorCommand;
pub use facade::DropTempFunctionCommand;
pub use facade::RenameTempConnectorCommand;
pub use facade::RenameTempFunctionCommand;
pub use facade::ExplainPlanCommand;
pub use facade::SetConfigCommand;
pub use facade::{
    ShowAlwaysDeletesCommand, ShowAlwaysUpdatesCommand, ShowDetailsCommand, ShowHistoryCommand, ShowStatusCommand, ShowViewsCommand,
    ShowIndexesCommand, ShowPacksCommand, ShowBlocksCommand, ShowConfigCommand,
    ShowCommandsCommand, ShowConnectorsCommand, ShowFunctionsCommand, ShowColumnsCommand,
    ShowCountCommand,
};
pub use facade::SyntaxCommand;
pub use facade::ProfileColumnCommand;

// Re-export extension traits
pub use facade_ext::BundleFacadeCommandExt;
pub use builder_ext::BundleBuilderExt;

/// Commands that can be executed on a BundleFacade (read-only).
///
/// This enum contains only commands that do not require mutation of the bundle.
/// It's a subset of `BundleCommand` that can be executed on a read-only `Bundle`.
#[derive(Debug, Clone)]
pub enum FacadeCommand {
    /// Export query results to a file
    ExportData(ExportDataCommand),
    /// Describe a registered connector's metadata
    DescribeConnector(DescribeConnectorCommand),
    /// Describe a registered function's metadata
    DescribeFunction(DescribeFunctionCommand),
    /// Load a temporary connector at runtime only (not persisted)
    ImportTempConnector(ImportTempConnectorCommand),
    /// Load a temporary function at runtime only (not persisted)
    ImportTempFunction(ImportTempFunctionCommand),
    /// Drop runtime-only connector (not persisted)
    DropTempConnector(DropTempConnectorCommand),
    /// Drop runtime-only function (not persisted)
    DropTempFunction(DropTempFunctionCommand),
    /// Rename runtime-only connector (not persisted)
    RenameTempConnector(RenameTempConnectorCommand),
    /// Rename runtime-only function (not persisted)
    RenameTempFunction(RenameTempFunctionCommand),
    /// Show query execution plan
    ExplainPlan(ExplainPlanCommand),
    /// Set runtime config value (session-only)
    SetConfig(SetConfigCommand),
    ShowAlwaysDeletes(ShowAlwaysDeletesCommand),
    ShowDetails(ShowDetailsCommand),
    ShowHistory(ShowHistoryCommand),
    ShowStatus(ShowStatusCommand),
    ShowViews(ShowViewsCommand),
    ShowIndexes(ShowIndexesCommand),
    ShowPacks(ShowPacksCommand),
    ShowBlocks(ShowBlocksCommand),
    ShowConfig(ShowConfigCommand),
    ShowCommands(ShowCommandsCommand),
    ShowConnectors(ShowConnectorsCommand),
    ShowFunctions(ShowFunctionsCommand),
    ShowColumns(ShowColumnsCommand),
    ShowCount(ShowCountCommand),
    /// Show syntax and usage for bundlebase commands
    Syntax(SyntaxCommand),
    /// Describe data quality and statistics for specified columns
    DescribeData(DescribeDataCommand),
    /// Test a connector without creating a source
    TestConnector(TestConnectorCommand),
    /// Profile a column's values (with optional cast-compatibility check)
    ProfileColumn(ProfileColumnCommand),
}

impl FacadeCommand {
    /// Execute this command on a BundleFacade, returning a boxed CommandResponse.
    pub async fn execute(
        self,
        facade: &dyn BundleFacade,
    ) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
        match self {
            FacadeCommand::ExportData(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::DescribeConnector(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::DescribeFunction(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ImportTempConnector(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ImportTempFunction(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::DropTempConnector(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::DropTempFunction(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::RenameTempConnector(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::RenameTempFunction(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ExplainPlan(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::SetConfig(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowAlwaysDeletes(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowDetails(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowHistory(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowStatus(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowViews(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowIndexes(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowPacks(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowBlocks(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowConfig(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowCommands(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowConnectors(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowFunctions(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowColumns(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ShowCount(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::Syntax(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::DescribeData(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::TestConnector(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
            FacadeCommand::ProfileColumn(cmd) => {
                let result = BundleFacadeCommand::execute(Box::new(cmd), facade).await?;
                Ok(Box::new(result))
            }
        }
    }

    /// Returns the Arrow schema for this command's output.
    pub fn output_schema(&self) -> SchemaRef {
        match self {
            FacadeCommand::ExportData(_) => ExportDataCommand::output_schema(),
            FacadeCommand::DescribeConnector(_) => DescribeConnectorCommand::output_schema(),
            FacadeCommand::DescribeFunction(_) => DescribeFunctionCommand::output_schema(),
            FacadeCommand::ImportTempConnector(_) => ImportTempConnectorCommand::output_schema(),
            FacadeCommand::ImportTempFunction(_) => ImportTempFunctionCommand::output_schema(),
            FacadeCommand::DropTempConnector(_) => DropTempConnectorCommand::output_schema(),
            FacadeCommand::DropTempFunction(_) => DropTempFunctionCommand::output_schema(),
            FacadeCommand::RenameTempConnector(_) => RenameTempConnectorCommand::output_schema(),
            FacadeCommand::RenameTempFunction(_) => RenameTempFunctionCommand::output_schema(),
            FacadeCommand::ExplainPlan(_) => ExplainPlanCommand::output_schema(),
            FacadeCommand::SetConfig(_) => SetConfigCommand::output_schema(),
            FacadeCommand::ShowAlwaysDeletes(_) => ShowAlwaysDeletesCommand::output_schema(),
            FacadeCommand::ShowDetails(_) => ShowDetailsCommand::output_schema(),
            FacadeCommand::ShowHistory(_) => ShowHistoryCommand::output_schema(),
            FacadeCommand::ShowStatus(_) => ShowStatusCommand::output_schema(),
            FacadeCommand::ShowViews(_) => ShowViewsCommand::output_schema(),
            FacadeCommand::ShowIndexes(_) => ShowIndexesCommand::output_schema(),
            FacadeCommand::ShowPacks(_) => ShowPacksCommand::output_schema(),
            FacadeCommand::ShowBlocks(_) => ShowBlocksCommand::output_schema(),
            FacadeCommand::ShowConfig(_) => ShowConfigCommand::output_schema(),
            FacadeCommand::ShowCommands(_) => ShowCommandsCommand::output_schema(),
            FacadeCommand::ShowConnectors(_) => ShowConnectorsCommand::output_schema(),
            FacadeCommand::ShowFunctions(_) => ShowFunctionsCommand::output_schema(),
            FacadeCommand::ShowColumns(_) => ShowColumnsCommand::output_schema(),
            FacadeCommand::ShowCount(_) => ShowCountCommand::output_schema(),
            FacadeCommand::Syntax(_) => SyntaxCommand::output_schema(),
            FacadeCommand::DescribeData(_) => DescribeDataCommand::output_schema(),
            FacadeCommand::TestConnector(_) => TestConnectorCommand::output_schema(),
            FacadeCommand::ProfileColumn(_) => ProfileColumnCommand::output_schema(),
        }
    }

    /// Returns the expected output shape for display formatting.
    pub fn output_shape(&self) -> OutputShape {
        match self {
            FacadeCommand::ExportData(_) => ExportDataCommand::output_shape(),
            FacadeCommand::DescribeConnector(_) => DescribeConnectorCommand::output_shape(),
            FacadeCommand::DescribeFunction(_) => DescribeFunctionCommand::output_shape(),
            FacadeCommand::ImportTempConnector(_) => ImportTempConnectorCommand::output_shape(),
            FacadeCommand::ImportTempFunction(_) => ImportTempFunctionCommand::output_shape(),
            FacadeCommand::DropTempConnector(_) => DropTempConnectorCommand::output_shape(),
            FacadeCommand::DropTempFunction(_) => DropTempFunctionCommand::output_shape(),
            FacadeCommand::RenameTempConnector(_) => RenameTempConnectorCommand::output_shape(),
            FacadeCommand::RenameTempFunction(_) => RenameTempFunctionCommand::output_shape(),
            FacadeCommand::ExplainPlan(_) => ExplainPlanCommand::output_shape(),
            FacadeCommand::SetConfig(_) => SetConfigCommand::output_shape(),
            FacadeCommand::ShowAlwaysDeletes(_) => ShowAlwaysDeletesCommand::output_shape(),
            FacadeCommand::ShowDetails(_) => ShowDetailsCommand::output_shape(),
            FacadeCommand::ShowHistory(_) => ShowHistoryCommand::output_shape(),
            FacadeCommand::ShowStatus(_) => ShowStatusCommand::output_shape(),
            FacadeCommand::ShowViews(_) => ShowViewsCommand::output_shape(),
            FacadeCommand::ShowIndexes(_) => ShowIndexesCommand::output_shape(),
            FacadeCommand::ShowPacks(_) => ShowPacksCommand::output_shape(),
            FacadeCommand::ShowBlocks(_) => ShowBlocksCommand::output_shape(),
            FacadeCommand::ShowConfig(_) => ShowConfigCommand::output_shape(),
            FacadeCommand::ShowCommands(_) => ShowCommandsCommand::output_shape(),
            FacadeCommand::ShowConnectors(_) => ShowConnectorsCommand::output_shape(),
            FacadeCommand::ShowFunctions(_) => ShowFunctionsCommand::output_shape(),
            FacadeCommand::ShowColumns(_) => ShowColumnsCommand::output_shape(),
            FacadeCommand::ShowCount(_) => ShowCountCommand::output_shape(),
            FacadeCommand::Syntax(_) => SyntaxCommand::output_shape(),
            FacadeCommand::DescribeData(_) => DescribeDataCommand::output_shape(),
            FacadeCommand::TestConnector(_) => TestConnectorCommand::output_shape(),
            FacadeCommand::ProfileColumn(_) => ProfileColumnCommand::output_shape(),
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
            BundleCommand::ExportData(cmd) => Ok(FacadeCommand::ExportData(cmd)),
            BundleCommand::DescribeConnector(cmd) => Ok(FacadeCommand::DescribeConnector(cmd)),
            BundleCommand::DescribeFunction(cmd) => Ok(FacadeCommand::DescribeFunction(cmd)),
            BundleCommand::ImportTempConnector(cmd) => Ok(FacadeCommand::ImportTempConnector(cmd)),
            BundleCommand::ImportTempFunction(cmd) => Ok(FacadeCommand::ImportTempFunction(cmd)),
            BundleCommand::DropTempConnector(cmd) => Ok(FacadeCommand::DropTempConnector(cmd)),
            BundleCommand::DropTempFunction(cmd) => Ok(FacadeCommand::DropTempFunction(cmd)),
            BundleCommand::RenameTempConnector(cmd) => Ok(FacadeCommand::RenameTempConnector(cmd)),
            BundleCommand::RenameTempFunction(cmd) => Ok(FacadeCommand::RenameTempFunction(cmd)),
            BundleCommand::ExplainPlan(cmd) => Ok(FacadeCommand::ExplainPlan(cmd)),
            BundleCommand::SetConfig(cmd) => Ok(FacadeCommand::SetConfig(cmd)),
            BundleCommand::ShowAlwaysDeletes(cmd) => Ok(FacadeCommand::ShowAlwaysDeletes(cmd)),
            BundleCommand::ShowDetails(cmd) => Ok(FacadeCommand::ShowDetails(cmd)),
            BundleCommand::ShowHistory(cmd) => Ok(FacadeCommand::ShowHistory(cmd)),
            BundleCommand::ShowStatus(cmd) => Ok(FacadeCommand::ShowStatus(cmd)),
            BundleCommand::ShowViews(cmd) => Ok(FacadeCommand::ShowViews(cmd)),
            BundleCommand::ShowIndexes(cmd) => Ok(FacadeCommand::ShowIndexes(cmd)),
            BundleCommand::ShowPacks(cmd) => Ok(FacadeCommand::ShowPacks(cmd)),
            BundleCommand::ShowBlocks(cmd) => Ok(FacadeCommand::ShowBlocks(cmd)),
            BundleCommand::ShowConfig(cmd) => Ok(FacadeCommand::ShowConfig(cmd)),
            BundleCommand::ShowCommands(cmd) => Ok(FacadeCommand::ShowCommands(cmd)),
            BundleCommand::ShowConnectors(cmd) => Ok(FacadeCommand::ShowConnectors(cmd)),
            BundleCommand::ShowFunctions(cmd) => Ok(FacadeCommand::ShowFunctions(cmd)),
            BundleCommand::ShowColumns(cmd) => Ok(FacadeCommand::ShowColumns(cmd)),
            BundleCommand::ShowCount(cmd) => Ok(FacadeCommand::ShowCount(cmd)),
            BundleCommand::Syntax(cmd) => Ok(FacadeCommand::Syntax(cmd)),
            BundleCommand::DescribeData(cmd) => Ok(FacadeCommand::DescribeData(cmd)),
            BundleCommand::TestConnector(cmd) => Ok(FacadeCommand::TestConnector(cmd)),
            BundleCommand::ProfileColumn(cmd) => Ok(FacadeCommand::ProfileColumn(cmd)),
            _ => {
                // Get the command name for the error message
                let cmd_name = match &self {
                    BundleCommand::AlwaysDelete(_) => "ALWAYS DELETE",
                    BundleCommand::AlwaysUpdate(_) => "ALWAYS UPDATE",
                    BundleCommand::Attach(_) => "ATTACH",
                    BundleCommand::Delete(_) => "DELETE",
                    BundleCommand::DropAlwaysDelete(_) => "DROP ALWAYS DELETE",
                    BundleCommand::DropAlwaysUpdate(_) => "DROP ALWAYS UPDATE",
                    BundleCommand::Update(_) => "UPDATE",
                    BundleCommand::DetachBlock(_) => "DETACH",
                    BundleCommand::Filter(_) => "FILTER",
                    BundleCommand::ImportJoin(_) => "IMPORT JOIN",
                    BundleCommand::Join(_) => "JOIN",
                    BundleCommand::ReplaceBlock(_) => "REPLACE",
                    BundleCommand::AddColumn(_) => "ADD COLUMN",
                    BundleCommand::CastColumn(_) => "CAST COLUMN",
                    BundleCommand::DropCastColumn(_) => "DROP CAST COLUMN",
                    BundleCommand::DropColumn(_) => "ALTER TABLE DROP COLUMN",
                    BundleCommand::RenameColumn(_) => "ALTER TABLE RENAME COLUMN",
                    BundleCommand::NormalizeColumnNames(_) => "NORMALIZE COLUMN NAMES",
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
                    BundleCommand::SaveConfig(_) => "SAVE CONFIG",
                    BundleCommand::ImportConnector(_) => "IMPORT CONNECTOR",
                    BundleCommand::ImportFunction(_) => "IMPORT FUNCTION",
                    BundleCommand::RenameConnector(_) => "RENAME CONNECTOR",
                    BundleCommand::RenameFunction(_) => "RENAME FUNCTION",
                    BundleCommand::DropConnector(_) => "DROP CONNECTOR",
                    BundleCommand::DropFunction(_) => "DROP FUNCTION",
                    BundleCommand::CreateSource(_) => "CREATE SOURCE",
                    BundleCommand::Reset(_) => "RESET",
                    BundleCommand::Undo(_) => "UNDO",
                    BundleCommand::Fetch(_) => "FETCH",
                    BundleCommand::FetchAll(_) => "FETCH ALL",
                    BundleCommand::VerifyData(_) => "VERIFY DATA",
                    BundleCommand::Commit(_) => "COMMIT",
                    BundleCommand::ExportHollow(_) => "EXPORT HOLLOW",
                    BundleCommand::ExportData(_) | BundleCommand::DescribeConnector(_) | BundleCommand::DescribeFunction(_) | BundleCommand::ImportTempConnector(_) | BundleCommand::ImportTempFunction(_) | BundleCommand::DropTempConnector(_) | BundleCommand::DropTempFunction(_) | BundleCommand::RenameTempConnector(_) | BundleCommand::RenameTempFunction(_) | BundleCommand::ExplainPlan(_) | BundleCommand::SetConfig(_) | BundleCommand::ShowAlwaysDeletes(_) | BundleCommand::ShowAlwaysUpdates(_) | BundleCommand::ShowDetails(_) | BundleCommand::ShowHistory(_) | BundleCommand::ShowStatus(_) | BundleCommand::ShowViews(_) | BundleCommand::ShowIndexes(_) | BundleCommand::ShowPacks(_) | BundleCommand::ShowBlocks(_) | BundleCommand::ShowConfig(_) | BundleCommand::ShowCommands(_) | BundleCommand::ShowConnectors(_) | BundleCommand::ShowFunctions(_) | BundleCommand::ShowColumns(_) | BundleCommand::ShowCount(_) | BundleCommand::Syntax(_) | BundleCommand::DescribeData(_) | BundleCommand::TestConnector(_) | BundleCommand::ProfileColumn(_) => {
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
        matches!(self, BundleCommand::ExportData(_) | BundleCommand::DescribeConnector(_) | BundleCommand::DescribeFunction(_) | BundleCommand::ImportTempConnector(_) | BundleCommand::ImportTempFunction(_) | BundleCommand::DropTempConnector(_) | BundleCommand::DropTempFunction(_) | BundleCommand::RenameTempConnector(_) | BundleCommand::RenameTempFunction(_) | BundleCommand::ExplainPlan(_) | BundleCommand::SetConfig(_) | BundleCommand::ShowAlwaysDeletes(_) | BundleCommand::ShowAlwaysUpdates(_) | BundleCommand::ShowDetails(_) | BundleCommand::ShowHistory(_) | BundleCommand::ShowStatus(_) | BundleCommand::ShowViews(_) | BundleCommand::ShowIndexes(_) | BundleCommand::ShowPacks(_) | BundleCommand::ShowBlocks(_) | BundleCommand::ShowConfig(_) | BundleCommand::ShowCommands(_) | BundleCommand::ShowConnectors(_) | BundleCommand::ShowFunctions(_) | BundleCommand::ShowColumns(_) | BundleCommand::ShowCount(_) | BundleCommand::Syntax(_) | BundleCommand::DescribeData(_) | BundleCommand::TestConnector(_) | BundleCommand::ProfileColumn(_))
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
/// These commands require access to a `BundleBuilder` and typically
/// apply operations that change the bundle's state.
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
/// - `available_commands()` function returning all command names and syntax strings
///
/// # Categories
///
/// - `message`: Commands using `execute_command()` returning String (boxed as `dyn CommandResponse`)
/// - `fetch_special`: Commands returning `Vec<FetchResults>` with special parsing (handled in parser.rs)
/// - `verification`: Commands returning `VerificationResults`
/// - `facade`: Read-only commands using `BundleFacadeCommand` (schema/shape from command struct)
///
/// Note: `fetch_special` commands are NOT included in `parse_from_rule()` because they share
/// grammar rules (e.g., fetch_stmt -> Fetch or FetchAll). They must be handled specially in parser.rs.
macro_rules! register_commands {
    (
        // Commands that return MessageResponse::ok()
        message {
            $( $msg_variant:ident($msg_cmd:ty) => $msg_rule:path, $msg_name:literal => $msg_syntax:literal ),* $(,)?
        }
        // Commands that return Vec<FetchResults> but need special parsing (shared rules)
        fetch_special {
            $( $fetch_variant:ident($fetch_cmd:ty), $fetch_name:literal => $fetch_syntax:literal ),* $(,)?
        }
        // Commands that return VerificationResults
        verification {
            $( $verify_variant:ident($verify_cmd:ty) => $verify_rule:path, $verify_name:literal => $verify_syntax:literal ),* $(,)?
        }
        // Read-only commands using BundleFacadeCommand (e.g. ExplainPlan, Show*)
        facade {
            $( $facade_variant:ident($facade_cmd:ty) => $facade_rule:path, $facade_name:literal => $facade_syntax:literal ),* $(,)?
        }
    ) => {
        /// Command that can be executed on a BundleBuilder.
        ///
        /// This enum wraps command structs, providing a single source of truth for command parameters.
        /// Each variant delegates to its wrapped command struct for execution.
        #[derive(Debug, Clone)]
        pub enum BundleCommand {
            // Message commands
            $( $msg_variant($msg_cmd), )*
            // Fetch commands (special parsing)
            $( $fetch_variant($fetch_cmd), )*
            // Verification commands
            $( $verify_variant($verify_cmd), )*
            // Facade commands (read-only)
            $( $facade_variant($facade_cmd), )*
        }

        impl BundleCommand {
            /// Execute this command on a BundleBuilder.
            ///
            /// This method delegates to the wrapped command struct via `run_command`,
            /// which handles change tracking. Facade commands bypass change tracking
            /// since they don't mutate state.
            pub async fn execute(self, builder: &BundleBuilder) -> Result<Box<dyn CommandResponse>, BundlebaseError> {
                match self {
                    // Message commands - return String boxed as CommandResponse
                    $(
                        BundleCommand::$msg_variant(cmd) => {
                            let description = cmd.to_statement();
                            let result = builder.run_command(description, Box::new(cmd).execute(builder)).await?;
                            Ok(Box::new(result) as Box<dyn CommandResponse>)
                        }
                    )*
                    // Fetch commands - return Vec<FetchResults> boxed
                    $(
                        BundleCommand::$fetch_variant(cmd) => {
                            let description = cmd.to_statement();
                            let results = builder.run_command(description, Box::new(cmd).execute(builder)).await?;
                            Ok(Box::new(results) as Box<dyn CommandResponse>)
                        }
                    )*
                    // Verification commands - return VerificationResults boxed
                    $(
                        BundleCommand::$verify_variant(cmd) => {
                            let description = cmd.to_statement();
                            let results = builder.run_command(description, Box::new(cmd).execute(builder)).await?;
                            Ok(Box::new(results) as Box<dyn CommandResponse>)
                        }
                    )*
                    // Facade commands - executed via BundleFacadeCommand trait (no change tracking)
                    $(
                        BundleCommand::$facade_variant(cmd) => {
                            let result = BundleFacadeCommand::execute(Box::new(cmd), builder).await?;
                            Ok(Box::new(result))
                        }
                    )*
                }
            }

            /// Returns the Arrow schema that this command will produce when executed.
            pub fn output_schema(&self) -> SchemaRef {
                match self {
                    // Fetch commands
                    $( BundleCommand::$fetch_variant(_) => Vec::<FetchResults>::schema(), )*
                    // Verification commands
                    $( BundleCommand::$verify_variant(_) => VerificationResults::schema(), )*
                    // Facade commands - schema from the command struct
                    $( BundleCommand::$facade_variant(_) => <$facade_cmd>::output_schema(), )*
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
                    // Facade commands - shape from the command struct
                    $( BundleCommand::$facade_variant(_) => <$facade_cmd>::output_shape(), )*
                    // All other commands return single value (OK message)
                    _ => String::output_shape(),
                }
            }

            /// Returns a map of command names to their syntax descriptions.
            ///
            /// This is auto-generated from the `register_commands!` macro invocation,
            /// ensuring every registered command has a syntax entry.
            pub fn available_commands() -> std::collections::HashMap<&'static str, &'static str> {
                let mut map = std::collections::HashMap::new();
                $( map.insert($msg_name, $msg_syntax); )*
                $( map.insert($fetch_name, $fetch_syntax); )*
                $( map.insert($verify_name, $verify_syntax); )*
                $( map.insert($facade_name, $facade_syntax); )*
                map
            }

            /// Returns metadata for all registered commands: (name, syntax, mode).
            ///
            /// Mode is "read-write" for builder commands and "read-only" for facade commands.
            /// Auto-generated from the `register_commands!` macro invocation.
            pub fn command_metadata() -> Vec<(&'static str, &'static str, &'static str)> {
                let mut entries = Vec::new();
                $( entries.push(($msg_name, $msg_syntax, "read-write")); )*
                $( entries.push(($fetch_name, $fetch_syntax, "read-write")); )*
                $( entries.push(($verify_name, $verify_syntax, "read-write")); )*
                $( entries.push(($facade_name, $facade_syntax, "read-only")); )*
                entries.sort_by_key(|(name, _, _)| name.to_string());
                entries
            }
        }

        /// Parse a command from a pest Rule and Pair.
        ///
        /// This function provides centralized rule-to-command mapping, ensuring
        /// that adding a command only requires updating the `register_commands!` macro.
        ///
        /// Note: Commands in `fetch_special` category are NOT handled here because they
        /// share grammar rules. Handle them in `parse_command()` directly.
        pub fn parse_from_rule(rule: Rule, pair: pest::iterators::Pair<Rule>) -> Result<Option<BundleCommand>, BundlebaseError> {
            match rule {
                // Message commands
                $( $msg_rule => Ok(Some(BundleCommand::$msg_variant(<$msg_cmd>::from_statement(pair)?))), )*
                // Note: fetch_special commands are handled in parser.rs, not here
                // Verification commands
                $( $verify_rule => Ok(Some(BundleCommand::$verify_variant(<$verify_cmd>::from_statement(pair)?))), )*
                // Facade commands
                $( $facade_rule => Ok(Some(BundleCommand::$facade_variant(<$facade_cmd>::from_statement(pair)?))), )*
                // Unknown rule - return None for special handling
                _ => Ok(None),
            }
        }
    };
}

// Register all commands using the macro.
//
// NOTE: Commands in `fetch_special` share the fetch_stmt rule and are handled
// specially in parser.rs::parse_command() rather than through parse_from_rule().
register_commands! {
    message {
        // Data modification commands
        AlwaysDelete(AlwaysDeleteCommand) => Rule::always_delete_stmt,
            "ALWAYS DELETE" => "ALWAYS DELETE FROM bundle WHERE <condition>",
        AlwaysUpdate(AlwaysUpdateCommand) => Rule::always_update_stmt,
            "ALWAYS UPDATE" => "ALWAYS UPDATE bundle SET <col> = <expr> [, ...] WHERE <condition>",
        Attach(AttachCommand) => Rule::attach_stmt,
            "ATTACH" => "ATTACH '<path>' [TO <pack>] [WITH (<options>)]",
        Delete(DeleteCommand) => Rule::delete_stmt,
            "DELETE" => "DELETE FROM bundle WHERE <condition>",
        DropAlwaysDelete(DropAlwaysDeleteCommand) => Rule::drop_always_delete_stmt,
            "DROP ALWAYS DELETE" => "DROP ALWAYS DELETE [WHERE <condition>]",
        DropAlwaysUpdate(DropAlwaysUpdateCommand) => Rule::drop_always_update_stmt,
            "DROP ALWAYS UPDATE" => "DROP ALWAYS UPDATE [SET <col> = <expr> [, ...] WHERE <condition>]",
        Update(UpdateCommand) => Rule::update_stmt,
            "UPDATE" => "UPDATE bundle SET <col> = <expr> [, ...] WHERE <condition>",
        DetachBlock(DetachBlockCommand) => Rule::detach_stmt,
            "DETACH" => "DETACH '<location>'",
        Filter(FilterCommand) => Rule::filter_stmt,
            "FILTER" => "FILTER WITH <select_query>",
        ImportJoin(ImportJoinCommand) => Rule::import_join_stmt,
            "IMPORT JOIN" => "IMPORT JOIN <name> [FLATTEN HISTORY]",
        Join(JoinCommand) => Rule::join_stmt,
            "JOIN" => "[LEFT|RIGHT|FULL|INNER] JOIN '<path>' AS <name> ON <expression>",
        ReplaceBlock(ReplaceBlockCommand) => Rule::replace_stmt,
            "REPLACE" => "REPLACE '<old_location>' WITH '<new_location>'",

        // Schema commands
        AddColumn(AddColumnCommand) => Rule::add_column_stmt,
            "ADD COLUMN" => "ADD COLUMN <name> AS <expression>",
        CastColumn(CastColumnCommand) => Rule::cast_column_stmt,
            "CAST COLUMN" => "CAST COLUMN <name> TO <type> [VERIFY EXISTING | NO VERIFY EXISTING]",
        DropCastColumn(DropCastColumnCommand) => Rule::drop_cast_column_stmt,
            "DROP CAST COLUMN" => "DROP CAST COLUMN <name>",
        DropColumn(DropColumnCommand) => Rule::drop_column_stmt,
            "DROP COLUMN" => "DROP COLUMN <name>",
        RenameColumn(RenameColumnCommand) => Rule::rename_column_stmt,
            "RENAME COLUMN" => "RENAME COLUMN <old> TO <new>",
        NormalizeColumnNames(NormalizeColumnNamesCommand) => Rule::normalize_column_names_stmt,
            "NORMALIZE COLUMN NAMES" => "NORMALIZE COLUMN NAMES",
        CreateIndex(CreateIndexCommand) => Rule::create_index_stmt,
            "CREATE INDEX" => "CREATE <COLUMN|TEXT> INDEX ON <column>",
        DropIndex(DropIndexCommand) => Rule::drop_index_stmt,
            "DROP INDEX" => "DROP INDEX <column>",
        RebuildIndex(RebuildIndexCommand) => Rule::rebuild_index_stmt,
            "REBUILD INDEX" => "REBUILD INDEX ON <column>",
        Reindex(ReindexCommand) => Rule::reindex_stmt,
            "REINDEX" => "REINDEX [ON data(<column>)]",

        // View commands
        CreateView(CreateViewCommand) => Rule::create_view_stmt,
            "CREATE VIEW" => "CREATE VIEW <name> AS <sql>",
        RenameView(RenameViewCommand) => Rule::rename_view_stmt,
            "RENAME VIEW" => "RENAME VIEW <old> TO <new>",
        DropView(DropViewCommand) => Rule::drop_view_stmt,
            "DROP VIEW" => "DROP VIEW <name>",

        // Join management commands
        DropJoin(DropJoinCommand) => Rule::drop_join_stmt,
            "DROP JOIN" => "DROP JOIN <name>",
        RenameJoin(RenameJoinCommand) => Rule::rename_join_stmt,
            "RENAME JOIN" => "RENAME JOIN <old> TO <new>",

        // Metadata commands
        SetName(SetNameCommand) => Rule::set_name_stmt,
            "SET NAME" => "SET NAME '<name>'",
        SetDescription(SetDescriptionCommand) => Rule::set_description_stmt,
            "SET DESCRIPTION" => "SET DESCRIPTION '<description>'",
        SaveConfig(SaveConfigCommand) => Rule::save_config_stmt,
            "SAVE CONFIG" => "SAVE CONFIG <key> = '<value>' FOR '<scope>'",

        // Source commands
        ImportConnector(ImportConnectorCommand) => Rule::import_connector_stmt,
            "IMPORT CONNECTOR" => "IMPORT CONNECTOR <name> FROM '<runtime::entrypoint>' [WITH (<args>)]",
        ImportFunction(ImportFunctionCommand) => Rule::import_function_stmt,
            "IMPORT FUNCTION" => "IMPORT FUNCTION <name> FROM '<runtime::entrypoint>' [WITH (<args>)]",
        RenameConnector(RenameConnectorCommand) => Rule::rename_connector_stmt,
            "RENAME CONNECTOR" => "RENAME CONNECTOR <old> TO <new>",
        RenameFunction(RenameFunctionCommand) => Rule::rename_function_stmt,
            "RENAME FUNCTION" => "RENAME FUNCTION <old> TO <new>",
        DropConnector(DropConnectorCommand) => Rule::drop_connector_stmt,
            "DROP CONNECTOR" => "DROP CONNECTOR <name> [FOR PLATFORM '<platform>']",
        DropFunction(DropFunctionCommand) => Rule::drop_function_stmt,
            "DROP FUNCTION" => "DROP FUNCTION <name>",
        CreateSource(CreateSourceCommand) => Rule::create_source_stmt,
            "CREATE SOURCE" => "CREATE SOURCE [FOR <pack>] USING <connector> [WITH (<args>)] [SAVE AS <AUTO|COPY|PARQUET|REF>]",

        // Transaction commands
        Reset(ResetCommand) => Rule::reset_stmt,
            "RESET" => "RESET",
        Undo(UndoCommand) => Rule::undo_stmt,
            "UNDO" => "UNDO",
        Commit(CommitCommand) => Rule::commit_stmt,
            "COMMIT" => "COMMIT '<message>'",

        // Export commands
        ExportHollow(ExportHollowCommand) => Rule::export_hollow_stmt,
            "EXPORT HOLLOW" => "EXPORT HOLLOW TO '<path>'",
    }
    fetch_special {
        // These commands share Rule::fetch_stmt - handled in parser.rs
        Fetch(FetchCommand),
            "FETCH" => "FETCH <pack> <ADD|UPDATE|SYNC> [DRY RUN]",
        FetchAll(FetchAllCommand),
            "FETCH ALL" => "FETCH ALL <ADD|UPDATE|SYNC> [DRY RUN]",
    }
    verification {
        VerifyData(VerifyDataCommand) => Rule::verify_data_stmt,
            "VERIFY DATA" => "VERIFY DATA [UPDATE]",
    }
    facade {
        ExportData(ExportDataCommand) => Rule::export_data_stmt,
            "EXPORT DATA" => "EXPORT DATA TO '<path>' <sql>",
        DescribeConnector(DescribeConnectorCommand) => Rule::describe_connector_stmt,
            "DESCRIBE CONNECTOR" => "DESCRIBE CONNECTOR <name>",
        DescribeFunction(DescribeFunctionCommand) => Rule::describe_function_stmt,
            "DESCRIBE FUNCTION" => "DESCRIBE FUNCTION <name>",
        ImportTempConnector(ImportTempConnectorCommand) => Rule::import_temp_connector_stmt,
            "IMPORT TEMP CONNECTOR" => "IMPORT TEMP CONNECTOR <name> FROM '<runtime::entrypoint>' [WITH (<args>)]",
        ImportTempFunction(ImportTempFunctionCommand) => Rule::import_temp_function_stmt,
            "IMPORT TEMP FUNCTION" => "IMPORT TEMP FUNCTION <name> FROM '<runtime::entrypoint>' [WITH (<args>)]",
        DropTempConnector(DropTempConnectorCommand) => Rule::drop_temp_connector_stmt,
            "DROP TEMP CONNECTOR" => "DROP TEMP CONNECTOR <name> [FOR PLATFORM '<platform>']",
        DropTempFunction(DropTempFunctionCommand) => Rule::drop_temp_function_stmt,
            "DROP TEMP FUNCTION" => "DROP TEMP FUNCTION <name>",
        RenameTempConnector(RenameTempConnectorCommand) => Rule::rename_temp_connector_stmt,
            "RENAME TEMP CONNECTOR" => "RENAME TEMP CONNECTOR <old> TO <new>",
        RenameTempFunction(RenameTempFunctionCommand) => Rule::rename_temp_function_stmt,
            "RENAME TEMP FUNCTION" => "RENAME TEMP FUNCTION <old> TO <new>",
        ExplainPlan(ExplainPlanCommand) => Rule::explain_stmt,
            "EXPLAIN" => "EXPLAIN [ANALYZE] [VERBOSE] [FORMAT <format>] [<sql>]",
        SetConfig(SetConfigCommand) => Rule::set_config_stmt,
            "SET CONFIG" => "SET CONFIG <key> = '<value>' FOR '<scope>'",
        ShowDetails(ShowDetailsCommand) => Rule::show_details_stmt,
            "SHOW DETAILS" => "SHOW DETAILS",
        ShowHistory(ShowHistoryCommand) => Rule::show_history_stmt,
            "SHOW HISTORY" => "SHOW HISTORY",
        ShowStatus(ShowStatusCommand) => Rule::show_status_stmt,
            "SHOW STATUS" => "SHOW STATUS",
        ShowViews(ShowViewsCommand) => Rule::show_views_stmt,
            "SHOW VIEWS" => "SHOW VIEWS",
        ShowIndexes(ShowIndexesCommand) => Rule::show_indexes_stmt,
            "SHOW INDEXES" => "SHOW INDEXES",
        ShowPacks(ShowPacksCommand) => Rule::show_packs_stmt,
            "SHOW PACKS" => "SHOW PACKS",
        ShowBlocks(ShowBlocksCommand) => Rule::show_blocks_stmt,
            "SHOW BLOCKS" => "SHOW BLOCKS",
        ShowConfig(ShowConfigCommand) => Rule::show_config_stmt,
            "SHOW CONFIG" => "SHOW CONFIG",
        ShowCommands(ShowCommandsCommand) => Rule::show_commands_stmt,
            "SHOW COMMANDS" => "SHOW COMMANDS",
        ShowConnectors(ShowConnectorsCommand) => Rule::show_connectors_stmt,
            "SHOW CONNECTORS" => "SHOW CONNECTORS",
        ShowFunctions(ShowFunctionsCommand) => Rule::show_functions_stmt,
            "SHOW FUNCTIONS" => "SHOW FUNCTIONS",
        ShowColumns(ShowColumnsCommand) => Rule::show_columns_stmt,
            "SHOW COLUMNS" => "SHOW COLUMNS",
        ShowCount(ShowCountCommand) => Rule::show_count_stmt,
            "SHOW COUNT" => "SHOW COUNT",
        ShowAlwaysDeletes(ShowAlwaysDeletesCommand) => Rule::show_always_deletes_stmt,
            "SHOW ALWAYS DELETES" => "SHOW ALWAYS DELETES",
        ShowAlwaysUpdates(ShowAlwaysUpdatesCommand) => Rule::show_always_updates_stmt,
            "SHOW ALWAYS UPDATES" => "SHOW ALWAYS UPDATES",
        Syntax(SyntaxCommand) => Rule::syntax_stmt,
            "SYNTAX" => "SYNTAX [<command>]",
        DescribeData(DescribeDataCommand) => Rule::describe_data_stmt,
            "DESCRIBE DATA" => "DESCRIBE DATA IN <col1> [AS <type>], <col2> [AS <type>], ...",
        TestConnector(TestConnectorCommand) => Rule::test_connector_stmt,
            "TEST CONNECTOR" => "TEST CONNECTOR <name> [WITH (<args>)] or TEST TEMP CONNECTOR '<runtime>::<entrypoint>' [WITH (<args>)]",
        ProfileColumn(ProfileColumnCommand) => Rule::profile_column_stmt,
            "PROFILE COLUMN" => "PROFILE COLUMN <name> [FOR CAST TO <type>]",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundlebase::source::SyncMode;
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
                assert_eq!(cmd.connector, "remote_dir");
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
                assert_eq!(cmd.connector, "remote_dir");
                assert_eq!(cmd.pack, Some("users".to_string()));
            }
            _ => panic!("Expected CreateSource variant"),
        }
    }

    #[test]
    fn test_fetch_command() {
        let cmd = BundleCommand::Fetch(FetchCommand::new("users".to_string(), SyncMode::Add));

        match cmd {
            BundleCommand::Fetch(cmd) => {
                assert_eq!(cmd.pack, "users");
            }
            _ => panic!("Expected Fetch variant"),
        }
    }

    #[test]
    fn test_fetch_all_command() {
        let cmd = BundleCommand::FetchAll(FetchAllCommand::new(SyncMode::Add));

        match cmd {
            BundleCommand::FetchAll(_) => {}
            _ => panic!("Expected FetchAll variant"),
        }
    }

}
