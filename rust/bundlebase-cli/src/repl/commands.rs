//! REPL commands module.
//!
//! This module contains all REPL commands, each in its own sub-module.
//! Each command module exports a `DEF` constant with metadata and an `execute` function.
//!
//! # Adding a new REPL command
//!
//! 1. Create a new module file (e.g., `my_command.rs`) with a `pub static DEF: ReplCommandDef`
//! 2. Add `pub mod my_command;` to the module declarations below
//! 3. Add a variant to the `ReplCommand` enum
//! 4. Add `&my_command::DEF` to the `all_commands()` array
//! 5. Add `Self::MyCommand => &my_command::DEF` to the `definition()` match

pub mod clear;
pub mod exit;
pub mod help;
mod sql;

use bundlebase::BundleFacade;
use bundlebase_command::parser::{parse_command, split_statements};
use bundlebase_command::{CommandResponse, OutputShape};
use bundlebase_common::BundlebaseError;
use datafusion::execution::SendableRecordBatchStream;
use futures;
use futures::future::BoxFuture;
use std::sync::Arc;

pub type ReplCommandResult =
    Result<Option<(SendableRecordBatchStream, OutputShape)>, BundlebaseError>;

/// Metadata for a repl command - all info about a command in one place
pub struct ReplCommandDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub usage: &'static str,
    pub create: fn(&str) -> Result<ReplCommand, String>,
    pub execute: fn(&ReplCommand, &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult>,
}

/// Repl commands
#[derive(Debug, Clone)]
pub enum ReplCommand {
    Clear,
    Exit,
    Help,
}

impl ReplCommand {
    /// Get command info - references module constants
    pub fn definition(&self) -> &'static ReplCommandDef {
        match self {
            Self::Clear => &clear::DEF,
            Self::Exit => &exit::DEF,
            Self::Help => &help::DEF,
        }
    }

    pub fn all_commands() -> impl Iterator<Item = &'static ReplCommandDef> {
        [&clear::DEF, &exit::DEF, &help::DEF].into_iter()
    }

    /// Parse from input string (without leading `/`)
    pub fn parse(input: &str) -> Result<Self, String> {
        let input = input.trim();
        let upper = input.to_uppercase();
        let first_word = upper.split_whitespace().next().unwrap_or("");
        let args = input.get(first_word.len()..).unwrap_or("").trim();

        // Find matching command using names
        for command_def in Self::all_commands() {
            if first_word == command_def.name.to_uppercase()
                || command_def
                    .aliases
                    .iter()
                    .any(|a| first_word == a.to_uppercase())
            {
                return (command_def.create)(args);
            }
        }

        Err(format!(
            "Unknown command: /{}. Type /help for available commands.",
            input
        ))
    }

    pub async fn execute(&self, bundle: &Arc<dyn BundleFacade>) -> ReplCommandResult {
        (self.definition().execute)(self, bundle).await
    }
}

/// Convert a CommandResponse to a stream
pub fn response_to_stream(
    response: Box<dyn CommandResponse>,
) -> Result<(SendableRecordBatchStream, OutputShape), BundlebaseError> {
    let shape = response.dyn_output_shape();
    let stream = response.into_stream()?;
    Ok((stream, shape))
}

#[derive(Debug, Clone)]
pub enum Command {
    /// SQL operations (executed via BundleFacade)
    Sql(String),

    Repl(ReplCommand),
}

/// Parse a single statement into a Command. Used internally by `parse`.
fn parse_single(input: &str) -> Result<Command, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Empty command".to_string());
    }

    if input.starts_with('/') {
        let repl_input = input[1..].trim();
        if repl_input.is_empty() {
            return Err("Empty command after '/'. Type /help for available commands.".to_string());
        }
        return ReplCommand::parse(repl_input).map(Command::Repl);
    }

    // Try to parse as SQL to validate syntax first
    // This gives better error messages for invalid SQL
    if let Err(e) = parse_command(input) {
        // Check if the user might have meant a repl command
        if let Some(suggestion) = suggest_repl_command(input) {
            return Err(format!(
                "Invalid SQL: {}. Did you mean '{}'?",
                e, suggestion
            ));
        }
        // If it's not a repl command suggestion and it's a syntax error,
        // it might be standard SQL (SELECT, etc.) - let it through
        let err_msg = e.to_string();
        if !err_msg.contains("Syntax error") {
            return Err(format!("Invalid SQL: {}", e));
        }
    }

    // Pass the raw SQL string to be executed via BundleState
    Ok(Command::Sql(input.to_string()))
}

/// Check if input looks like a bare repl command and suggest the `/` prefix
fn suggest_repl_command(input: &str) -> Option<String> {
    let upper = input.to_uppercase();
    let first_word = upper.split_whitespace().next().unwrap_or("");

    // Check against all repl commands
    for info in ReplCommand::all_commands() {
        if first_word == info.name.to_uppercase() {
            return Some(format!("/{}", info.name));
        }
        for alias in info.aliases {
            if first_word == alias.to_uppercase() {
                return Some(format!("/{}", alias));
            }
        }
    }
    None
}

/// Parse input that may contain multiple semicolon-separated statements.
///
/// Uses a grammar-based parser to split on `;` while respecting quoted strings.
/// All statements are validated before any are returned. If any statement fails
/// to parse, an error is returned and none are executed.
pub fn parse(input: &str) -> Result<Vec<Command>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Empty command".to_string());
    }

    // REPL commands cannot be part of multi-statement input
    if input.starts_with('/') {
        return Ok(vec![parse_single(input)?]);
    }

    let parts = split_statements(input).map_err(|e| e.to_string())?;
    if parts.is_empty() {
        return Err("Empty command".to_string());
    }

    // Parse and validate all statements before returning any
    let mut commands = Vec::with_capacity(parts.len());
    let mut errors = Vec::new();

    for (i, stmt) in parts.iter().enumerate() {
        match parse_single(stmt) {
            Ok(cmd) => commands.push(cmd),
            Err(e) => {
                if parts.len() == 1 {
                    errors.push(e);
                } else {
                    errors.push(format!("Statement {}: {}", i + 1, e));
                }
            }
        }
    }

    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    Ok(commands)
}

/// Get SQL command suggestions (for tab completion)
pub fn get_parameter_names(_command_name: &str) -> Vec<String> {
    // With SQL syntax, we don't need parameter completion
    // This function is kept for compatibility but returns empty
    vec![]
}

/// Execute a command, returning a stream and output shape (or None for
/// Exit/Clear). Applies the interactive REPL row cap.
pub async fn execute(cmd: Command, bundle: &Arc<dyn BundleFacade>) -> ReplCommandResult {
    execute_with_hard_limit(cmd, bundle, Some(sql::CLI_QUERY_LIMIT)).await
}

/// Execute a command with an explicit hard row limit (or `None` for
/// unlimited). One-shot CLI invocations use `None` so scripts get every
/// row their SQL asked for.
pub async fn execute_with_hard_limit(
    cmd: Command,
    bundle: &Arc<dyn BundleFacade>,
    hard_limit: Option<usize>,
) -> ReplCommandResult {
    match cmd {
        Command::Sql(sql_str) => {
            let (stream, shape) =
                sql::execute_with_hard_limit(bundle, &sql_str, hard_limit).await?;
            Ok(Some((stream, shape)))
        }
        Command::Repl(repl_cmd) => repl_cmd.execute(bundle).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to parse a single statement and return the first command.
    fn parse_one(input: &str) -> Result<Command, String> {
        let cmds = parse(input)?;
        assert_eq!(cmds.len(), 1, "Expected 1 command, got {}", cmds.len());
        Ok(cmds.into_iter().next().unwrap())
    }

    #[test]
    fn test_parse_attach() {
        let cmd = parse_one("ATTACH 'data.parquet'").unwrap();
        match cmd {
            Command::Sql(sql) => {
                assert!(sql.contains("ATTACH"));
                assert!(sql.contains("data.parquet"));
            }
            _ => panic!("Expected Sql command"),
        }
    }

    #[test]
    fn test_parse_filter() {
        let cmd = parse_one("FILTER WHERE country = 'USA'").unwrap();
        match cmd {
            Command::Sql(sql) => {
                assert!(sql.contains("FILTER"));
                assert!(sql.contains("country = 'USA'"));
            }
            _ => panic!("Expected Sql command"),
        }
    }

    #[test]
    fn test_parse_repl_commands() {
        assert!(matches!(
            parse_one("/help").unwrap(),
            Command::Repl(ReplCommand::Help)
        ));
        assert!(matches!(
            parse_one("/exit").unwrap(),
            Command::Repl(ReplCommand::Exit)
        ));
        assert!(matches!(
            parse_one("/quit").unwrap(),
            Command::Repl(ReplCommand::Exit)
        ));
        assert!(matches!(
            parse_one("/clear").unwrap(),
            Command::Repl(ReplCommand::Clear)
        ));
    }

    #[test]
    fn test_parse_repl_commands_case_insensitive() {
        assert!(matches!(
            parse_one("/HELP").unwrap(),
            Command::Repl(ReplCommand::Help)
        ));
        assert!(matches!(
            parse_one("/Help").unwrap(),
            Command::Repl(ReplCommand::Help)
        ));
        assert!(matches!(
            parse_one("/EXIT").unwrap(),
            Command::Repl(ReplCommand::Exit)
        ));
    }

    #[test]
    fn test_parse_repl_commands_with_space_after_slash() {
        assert!(matches!(
            parse_one("/ help").unwrap(),
            Command::Repl(ReplCommand::Help)
        ));
    }

    #[test]
    fn test_bare_repl_command_errors_with_suggestion() {
        let result = parse("HELP");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("/help"), "Error should suggest /help: {}", err);
    }

    #[test]
    fn test_show_is_sql_not_repl() {
        let cmd = parse_one("SHOW HISTORY").unwrap();
        assert!(matches!(cmd, Command::Sql(_)));
    }

    #[test]
    fn test_unknown_repl_command() {
        let result = parse("/foo");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Unknown command: /foo"), "Error: {}", err);
        assert!(err.contains("/help"), "Error should suggest /help: {}", err);
    }

    #[test]
    fn test_empty_repl_command() {
        let result = parse("/");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Empty command"), "Error: {}", err);
    }

    #[test]
    fn test_parse_commit() {
        let cmd = parse_one("COMMIT 'my commit message'").unwrap();
        match cmd {
            Command::Sql(sql) => {
                assert!(sql.contains("COMMIT"));
                assert!(sql.contains("my commit message"));
            }
            _ => panic!("Expected Sql command"),
        }
    }

    // Multi-statement tests

    #[test]
    fn test_parse_two_statements() {
        let cmds = parse("ATTACH 'data.csv'; SHOW HISTORY").unwrap();
        assert_eq!(cmds.len(), 2);
        assert!(matches!(&cmds[0], Command::Sql(_)));
        assert!(matches!(&cmds[1], Command::Sql(_)));
    }

    #[test]
    fn test_parse_trailing_semicolon() {
        let cmds = parse("SHOW HISTORY;").unwrap();
        assert_eq!(cmds.len(), 1);
    }

    #[test]
    fn test_parse_semicolon_in_quotes() {
        let cmds = parse("COMMIT 'msg with ; in it'; SHOW STATUS").unwrap();
        assert_eq!(cmds.len(), 2);
        match &cmds[0] {
            Command::Sql(sql) => assert!(sql.contains("msg with ; in it")),
            _ => panic!("Expected Sql"),
        }
    }

    #[test]
    fn test_parse_validates_all_before_returning() {
        // Second statement is invalid — HELP is not SQL, should suggest /help
        let result = parse("SHOW HISTORY; HELP");
        assert!(result.is_err(), "Should fail validation: {:?}", result);
        let err = result.unwrap_err();
        assert!(
            err.contains("Statement 2"),
            "Error should reference statement number: {}",
            err
        );
    }
}
