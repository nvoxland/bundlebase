//! Command parsing module.
//!
//! This module provides the entry point for parsing command statements into `BundleCommand`.
//!
//! # Architecture
//!
//! The parser uses a Pest grammar to handle all bundlebase commands (FILTER, ATTACH, JOIN, etc.).
//!
//! Each command struct implements parsing methods via the `CommandParsing` trait:
//! - `rule()` - Returns the pest Rule for this command
//! - `from_statement(pair)` - Parses from a pest Pair
//! - `to_statement()` - Serializes back to command string (round-trip support)

mod pest_parser;

// Re-export pest parser infrastructure
pub use pest_parser::{
    escape_string, extract_string_content, format_pest_error, parse_join_type, BundlebaseParser,
    Rule,
};

use crate::bundle::command::{
    AttachCommand, BundleCommand, CommandParsing, CommitCommand, CreateIndexCommand, CreateSourceCommand,
    DetachBlockCommand, DropColumnCommand, DropIndexCommand, DropJoinCommand, DropViewCommand,
    FetchAllCommand, FetchCommand, FilterCommand, JoinCommand, RebuildIndexCommand, ReindexCommand,
    RenameColumnCommand, RenameJoinCommand, RenameViewCommand, ReplaceBlockCommand, ResetCommand,
    SelectCommand, SetConfigCommand, SetDescriptionCommand, SetNameCommand, UndoCommand,
    VerifyDataCommand,
};
use crate::BundlebaseError;
use pest::Parser;

/// Parse a command statement into a BundleCommand.
///
/// This is the main entry point for parsing command statements into BundleCommand that can be
/// executed on a BundleBuilder.
///
/// # Arguments
///
/// * `command_str` - The command statement string to parse
///
/// # Returns
///
/// * `Ok(BundleCommand)` - Successfully parsed command
/// * `Err(BundlebaseError)` - Parsing failed or statement type not supported
///
/// # Examples
///
/// ```ignore
/// use bundlebase::bundle::{parse_command, BundleCommand};
///
/// // Parse a FILTER statement
/// let cmd = parse_command("FILTER WHERE country = 'USA'").unwrap();
///
/// // Parse a SELECT statement
/// let cmd = parse_command("SELECT name, email FROM bundle").unwrap();
///
/// // Parse an ATTACH statement
/// let cmd = parse_command("ATTACH 'data.parquet'").unwrap();
///
/// // Execute on a BundleBuilder
/// cmd.execute(&mut bundle).await?;
/// ```
pub fn parse_command(command_str: &str) -> Result<BundleCommand, BundlebaseError> {
    let mut pairs = BundlebaseParser::parse(Rule::statement, command_str)
        .map_err(|e| format_pest_error(e, command_str))?;

    // Get the top-level statement rule
    let statement = pairs
        .next()
        .ok_or_else(|| BundlebaseError::from("Parser produced empty result"))?;

    // Get the category rule (data_modification_stmt, schema_stmt, etc.)
    let category_stmt = statement
        .into_inner()
        .next()
        .ok_or_else(|| BundlebaseError::from("Parser produced empty inner statement"))?;

    // Get the actual statement type from the category
    let inner_stmt = category_stmt
        .into_inner()
        .next()
        .ok_or_else(|| BundlebaseError::from("Parser produced empty statement in category"))?;

    match inner_stmt.as_rule() {
        Rule::filter_stmt => Ok(BundleCommand::Filter(FilterCommand::from_statement(inner_stmt)?)),
        Rule::attach_stmt => Ok(BundleCommand::Attach(AttachCommand::from_statement(inner_stmt)?)),
        Rule::join_stmt => Ok(BundleCommand::Join(JoinCommand::from_statement(inner_stmt)?)),
        Rule::reindex_stmt => Ok(BundleCommand::Reindex(ReindexCommand::from_statement(inner_stmt)?)),
        Rule::create_source_stmt => Ok(BundleCommand::CreateSource(
            CreateSourceCommand::from_statement(inner_stmt)?,
        )),
        Rule::fetch_stmt => {
            // FETCH can be either FetchCommand or FetchAllCommand
            let raw = inner_stmt.as_str().to_uppercase();
            if raw.contains("ALL") {
                Ok(BundleCommand::FetchAll(FetchAllCommand::from_statement(
                    inner_stmt,
                )?))
            } else {
                Ok(BundleCommand::Fetch(FetchCommand::from_statement(inner_stmt)?))
            }
        }
        Rule::drop_join_stmt => Ok(BundleCommand::DropJoin(DropJoinCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::rename_join_stmt => Ok(BundleCommand::RenameJoin(RenameJoinCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::select_stmt => Ok(BundleCommand::Select(SelectCommand::from_statement(inner_stmt)?)),
        Rule::drop_index_stmt => Ok(BundleCommand::DropIndex(DropIndexCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::rename_view_stmt => Ok(BundleCommand::RenameView(RenameViewCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::reset_stmt => Ok(BundleCommand::Reset(ResetCommand::from_statement(inner_stmt)?)),
        Rule::undo_stmt => Ok(BundleCommand::Undo(UndoCommand::from_statement(inner_stmt)?)),
        Rule::commit_stmt => Ok(BundleCommand::Commit(CommitCommand::from_statement(inner_stmt)?)),
        Rule::detach_stmt => Ok(BundleCommand::DetachBlock(DetachBlockCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::rebuild_index_stmt => Ok(BundleCommand::RebuildIndex(
            RebuildIndexCommand::from_statement(inner_stmt)?,
        )),
        Rule::set_config_stmt => Ok(BundleCommand::SetConfig(SetConfigCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::replace_stmt => Ok(BundleCommand::ReplaceBlock(ReplaceBlockCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::set_name_stmt => Ok(BundleCommand::SetName(SetNameCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::set_description_stmt => Ok(BundleCommand::SetDescription(
            SetDescriptionCommand::from_statement(inner_stmt)?,
        )),
        Rule::drop_view_stmt => Ok(BundleCommand::DropView(DropViewCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::create_index_stmt => Ok(BundleCommand::CreateIndex(CreateIndexCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::drop_column_stmt => Ok(BundleCommand::DropColumn(DropColumnCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::rename_column_stmt => Ok(BundleCommand::RenameColumn(
            RenameColumnCommand::from_statement(inner_stmt)?,
        )),
        Rule::verify_data_stmt => Ok(BundleCommand::VerifyData(VerifyDataCommand::from_statement(
            inner_stmt,
        )?)),
        Rule::create_view_stmt => Err(
            "CREATE VIEW cannot be parsed from SQL. Use builder.create_view() API instead.".into(),
        ),
        _ => Err("Unexpected statement type".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sql_empty() {
        let result = parse_command("");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Syntax error"));
    }

    #[test]
    fn test_parse_select_captures_full_statement() {
        // Pest grammar captures the full SELECT statement including any trailing content.
        // DataFusion will validate the SQL syntax when executed.
        let result = parse_command("SELECT * FROM bundle; SELECT * FROM bundle2;");
        assert!(result.is_ok());
        match result.unwrap() {
            BundleCommand::Select(cmd) => {
                // The full input is captured as the SQL string
                assert!(cmd.sql.contains("bundle"));
            }
            _ => panic!("Expected Select variant"),
        }
    }
}
