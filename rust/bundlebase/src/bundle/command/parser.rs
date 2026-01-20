//! Command parsing module.
//!
//! This module provides the entry point for parsing command statements into `BundleCommand`.
//!
//! # Architecture
//!
//! The parser uses a two-stage approach:
//! 1. **Pest grammar** for bundlebase-specific syntax (FILTER, ATTACH, JOIN, etc.)
//! 2. **sqlparser-rs** for standard SQL (CREATE INDEX, etc.)
//!
//! Each command struct implements parsing methods via the `Command` trait:
//! - `rule()` - Returns the pest Rule for this command
//! - `from_pest(pair)` - Parses from a pest Pair
//! - `to_statement()` - Serializes back to command string (round-trip support)

mod pest_parser;

// Re-export pest parser infrastructure
pub use pest_parser::{
    escape_string, extract_string_content, format_pest_error, is_likely_custom_syntax,
    parse_join_type, BundlebaseParser, Rule,
};

use crate::bundle::command::{
    AttachCommand, BundleCommand, Command, CommitCommand, CreateIndexCommand, CreateSourceCommand,
    DetachBlockCommand, DropColumnCommand, DropIndexCommand, DropJoinCommand, DropViewCommand,
    FetchAllCommand, FetchCommand, FilterCommand, JoinCommand, RebuildIndexCommand, ReindexCommand,
    RenameColumnCommand, RenameJoinCommand, RenameViewCommand, ReplaceBlockCommand, ResetCommand,
    SelectCommand, SetConfigCommand, SetDescriptionCommand, SetNameCommand, UndoCommand,
    VerifyDataCommand,
};
use crate::BundlebaseError;
use pest::Parser;
use sqlparser::ast::Statement;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser as SqlParser;

/// Parse a command statement into a BundleCommand.
///
/// This is the main entry point for parsing command statements into BundleCommand that can be
/// executed on a BundleBuilder.
///
/// It handles:
/// 1. Parsing custom bundlebase syntax (FILTER, ATTACH, JOIN, REINDEX) using Pest
/// 2. Parsing standard SQL (SELECT, CREATE INDEX, etc.) using sqlparser-rs
/// 3. Converting parsed statements into BundleCommand variants
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
    // First, try Pest grammar for bundlebase syntax (FILTER, ATTACH, JOIN, SELECT, etc.)
    if let Some(cmd) = try_parse_pest(command_str)? {
        return Ok(cmd);
    }

    // Otherwise, use sqlparser-rs for standard SQL (CREATE INDEX, etc.)
    let dialect = GenericDialect {};
    let ast = SqlParser::parse_sql(&dialect, command_str)
        .map_err(|e| -> BundlebaseError { format!("SQL parse error: {}", e).into() })?;

    if ast.is_empty() {
        return Err("Empty SQL statement".into());
    }

    if ast.len() > 1 {
        return Err(
            "Multiple statements not supported. Please execute one statement at a time.".into(),
        );
    }

    let stmt = &ast[0];

    // Dispatch to appropriate operation based on statement type
    dispatch_statement(stmt)
}

/// Try to parse using Pest grammar.
///
/// Returns Ok(Some(cmd)) if successfully parsed, Ok(None) if not custom syntax,
/// or Err if it looks like custom syntax but failed to parse.
fn try_parse_pest(sql: &str) -> Result<Option<BundleCommand>, BundlebaseError> {
    let parse_result = BundlebaseParser::parse(Rule::statement, sql);

    match parse_result {
        Ok(mut pairs) => {
            // Get the top-level statement rule
            let statement = pairs
                .next()
                .ok_or_else(|| BundlebaseError::from("Parser produced empty result"))?;

            // Get the inner statement type (filter_stmt, attach_stmt, etc.)
            let inner_stmt = statement
                .into_inner()
                .next()
                .ok_or_else(|| BundlebaseError::from("Parser produced empty inner statement"))?;

            let cmd = match inner_stmt.as_rule() {
                Rule::filter_stmt => BundleCommand::Filter(FilterCommand::from_pest(inner_stmt)?),
                Rule::attach_stmt => BundleCommand::Attach(AttachCommand::from_pest(inner_stmt)?),
                Rule::join_stmt => BundleCommand::Join(JoinCommand::from_pest(inner_stmt)?),
                Rule::reindex_stmt => BundleCommand::Reindex(ReindexCommand::from_pest(inner_stmt)?),
                Rule::create_source_stmt => {
                    BundleCommand::CreateSource(CreateSourceCommand::from_pest(inner_stmt)?)
                }
                Rule::fetch_stmt => {
                    // FETCH can be either FetchCommand or FetchAllCommand
                    // Check if it's FETCH ALL
                    let raw = inner_stmt.as_str().to_uppercase();
                    if raw.contains("ALL") {
                        BundleCommand::FetchAll(FetchAllCommand::from_pest(inner_stmt)?)
                    } else {
                        BundleCommand::Fetch(FetchCommand::from_pest(inner_stmt)?)
                    }
                }
                Rule::drop_join_stmt => {
                    BundleCommand::DropJoin(DropJoinCommand::from_pest(inner_stmt)?)
                }
                Rule::rename_join_stmt => {
                    BundleCommand::RenameJoin(RenameJoinCommand::from_pest(inner_stmt)?)
                }
                Rule::select_stmt => BundleCommand::Select(SelectCommand::from_pest(inner_stmt)?),
                Rule::drop_index_stmt => {
                    BundleCommand::DropIndex(DropIndexCommand::from_pest(inner_stmt)?)
                }
                Rule::rename_view_stmt => {
                    BundleCommand::RenameView(RenameViewCommand::from_pest(inner_stmt)?)
                }
                Rule::reset_stmt => BundleCommand::Reset(ResetCommand::from_pest(inner_stmt)?),
                Rule::undo_stmt => BundleCommand::Undo(UndoCommand::from_pest(inner_stmt)?),
                Rule::commit_stmt => BundleCommand::Commit(CommitCommand::from_pest(inner_stmt)?),
                Rule::detach_stmt => {
                    BundleCommand::DetachBlock(DetachBlockCommand::from_pest(inner_stmt)?)
                }
                Rule::rebuild_index_stmt => {
                    BundleCommand::RebuildIndex(RebuildIndexCommand::from_pest(inner_stmt)?)
                }
                Rule::set_config_stmt => {
                    BundleCommand::SetConfig(SetConfigCommand::from_pest(inner_stmt)?)
                }
                Rule::replace_stmt => {
                    BundleCommand::ReplaceBlock(ReplaceBlockCommand::from_pest(inner_stmt)?)
                }
                Rule::set_name_stmt => {
                    BundleCommand::SetName(SetNameCommand::from_pest(inner_stmt)?)
                }
                Rule::set_description_stmt => {
                    BundleCommand::SetDescription(SetDescriptionCommand::from_pest(inner_stmt)?)
                }
                Rule::drop_view_stmt => {
                    BundleCommand::DropView(DropViewCommand::from_pest(inner_stmt)?)
                }
                Rule::create_index_stmt => {
                    BundleCommand::CreateIndex(CreateIndexCommand::from_pest(inner_stmt)?)
                }
                Rule::drop_column_stmt => {
                    BundleCommand::DropColumn(DropColumnCommand::from_pest(inner_stmt)?)
                }
                Rule::rename_column_stmt => {
                    BundleCommand::RenameColumn(RenameColumnCommand::from_pest(inner_stmt)?)
                }
                Rule::verify_data_stmt => {
                    BundleCommand::VerifyData(VerifyDataCommand::from_pest(inner_stmt)?)
                }
                Rule::create_view_stmt => {
                    return Err("CREATE VIEW cannot be parsed from SQL. Use builder.create_view() API instead.".into());
                }
                _ => return Err("Unexpected statement type".into()),
            };
            Ok(Some(cmd))
        }
        Err(e) => {
            // Not custom syntax or parse error
            if is_likely_custom_syntax(sql) {
                // If it looks like custom syntax but failed to parse, report error
                Err(format_pest_error(e, sql))
            } else {
                // Not custom syntax, return None to let sqlparser handle it
                Ok(None)
            }
        }
    }
}

/// Dispatch a SQL statement to the appropriate BundleCommand.
///
/// This function examines the statement type and creates the appropriate BundleCommand variant
/// for statements not handled by pest grammar.
///
/// Note: Most statements (SELECT, DROP INDEX, RENAME VIEW, etc.) are now handled by pest.
/// This function only handles legacy/rare SQL statements.
fn dispatch_statement(stmt: &Statement) -> Result<BundleCommand, BundlebaseError> {
    match stmt {
        // CREATE INDEX -> Index
        Statement::CreateIndex { .. } => {
            // sqlparser 0.59 changed CreateIndex structure
            // For now, return error - use REINDEX or custom INDEX commands instead
            Err("CREATE INDEX via standard SQL is not yet supported. Use bundlebase INDEX command or REINDEX.".into())
        }

        // Unrecognized statement types
        _ => Err(format!("Unsupported SQL statement type: {:?}", stmt).into()),
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
        assert!(err_msg.contains("Empty"));
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
