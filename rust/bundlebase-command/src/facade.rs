//! Facade command implementations.
//!
//! This module contains command implementations for read-only commands
//! that work with `&dyn BundleFacade`.

// Facade command implementations
mod describe_connector;
pub mod describe_data;
mod describe_function;
mod drop_temp_connector;
mod drop_temp_function;
mod explain;
mod export_data;
mod generate_report;
mod import_temp_connector;
mod import_temp_function;
mod profile_column;
mod rename_temp_connector;
mod rename_temp_function;
mod set_config;
mod show;
mod show_count;
mod syntax;
mod test_connector;

pub use describe_connector::DescribeConnectorCommand;
pub use describe_data::DescribeDataCommand;
pub use describe_function::DescribeFunctionCommand;
pub use drop_temp_connector::DropTempConnectorCommand;
pub use drop_temp_function::DropTempFunctionCommand;
pub use explain::ExplainPlanCommand;
pub use export_data::ExportDataCommand;
pub use generate_report::GenerateReportCommand;
pub use import_temp_connector::ImportTempConnectorCommand;
pub use import_temp_function::ImportTempFunctionCommand;
pub use profile_column::ProfileColumnCommand;
pub use rename_temp_connector::RenameTempConnectorCommand;
pub use rename_temp_function::RenameTempFunctionCommand;
pub use set_config::SetConfigCommand;
pub use show::{
    ShowAlwaysDeletesCommand, ShowAlwaysUpdatesCommand, ShowBlocksCommand, ShowColumnsCommand,
    ShowCommandsCommand, ShowConfigCommand, ShowConnectorsCommand, ShowDetailsCommand,
    ShowFunctionsCommand, ShowHistoryCommand, ShowIndexesCommand, ShowPacksCommand,
    ShowReportsCommand, ShowSourcesCommand, ShowStatusCommand, ShowViewsCommand,
};
pub use show_count::ShowCountCommand;
pub use syntax::SyntaxCommand;
pub use test_connector::TestConnectorCommand;
