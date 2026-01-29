//! REPL commands module.
//!
//! This module contains all REPL commands, each in its own sub-module.
//! Each command module exports an `INFO` constant with metadata and an `execute` function.

pub mod clear;
pub mod count;
pub mod details;
pub mod exit;
pub mod help;
pub mod history;
pub mod schema;
pub mod show;
mod sql;
pub mod status;

use bundlebase::bundle::{parse_command, CommandResponse, OutputShape};
use bundlebase::BundlebaseError;
use bundlebase::BundleFacade;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use futures;
use futures::future::BoxFuture;
use std::sync::Arc;

pub type ReplCommandResult = Result<Option<(SendableRecordBatchStream, OutputShape)>, BundlebaseError>;

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
    Count,
    Details,
    Exit,
    Help,
    History,
    Schema,
    Show { limit: Option<usize> },
    Status,
}

impl ReplCommand {
    /// Get command info - references module constants
    pub fn definition(&self) -> &'static ReplCommandDef {
        match self {
            Self::Clear => &clear::DEF,
            Self::Count => &count::DEF,
            Self::Details => &details::DEF,
            Self::Exit => &exit::DEF,
            Self::Help => &help::DEF,
            Self::History => &history::DEF,
            Self::Schema => &schema::DEF,
            Self::Show { .. } => &show::DEF,
            Self::Status => &status::DEF,
        }
    }

    pub fn all_commands() -> impl Iterator<Item = &'static ReplCommandDef> {
        [
            &clear::DEF,
            &count::DEF,
            &details::DEF,
            &exit::DEF,
            &help::DEF,
            &history::DEF,
            &schema::DEF,
            &show::DEF,
            &status::DEF,
        ]
        .into_iter()
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
                || command_def.aliases.iter().any(|a| first_word == a.to_uppercase())
            {
                return (command_def.create)(args);
            }
        }

        Err(format!(
            "Unknown command: /{}. Type /help for available commands.",
            input
        ))
    }

    pub async fn execute(
        &self,
        bundle: &Arc<dyn BundleFacade>,
    ) -> ReplCommandResult {
        (self.definition().execute)(self, bundle).await
    }
}

/// Convert a CommandResponse to a stream
pub fn response_to_stream(
    response: &dyn CommandResponse,
) -> Result<(SendableRecordBatchStream, OutputShape), BundlebaseError> {
    let schema = response.dyn_schema();
    let shape = response.dyn_output_shape();
    let batch = response.to_record_batch()?;
    let stream = Box::pin(RecordBatchStreamAdapter::new(
        schema,
        futures::stream::iter(vec![Ok(batch)]),
    ));
    Ok((stream, shape))
}

#[derive(Debug, Clone)]
pub enum Command {
    /// SQL operations (executed via BundleFacade)
    Sql(String),

    Repl(ReplCommand),
}

/// Parse input string into Command using SQL syntax
/// Repl commands start with `/` (e.g., `/help`, `/show`)
/// All other input is treated as SQL
pub fn parse(input: &str) -> Result<Command, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Empty command".to_string());
    }

    if input.starts_with('/') {
        let repl_input = input[1..].trim();
        if repl_input.is_empty() {
            return Err(
                "Empty command after '/'. Type /help for available commands.".to_string(),
            );
        }
        return ReplCommand::parse(repl_input).map(Command::Repl);
    }

    // Try to parse as SQL to validate syntax first
    // This gives better error messages for invalid SQL
    if let Err(e) = parse_command(input) {
        // Check if the user might have meant a repl command
        if let Some(suggestion) = suggest_repl_command(input) {
            return Err(format!("Invalid SQL: {}. Did you mean '{}'?", e, suggestion));
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

/// Get SQL command suggestions (for tab completion)
pub fn get_parameter_names(_command_name: &str) -> Vec<String> {
    // With SQL syntax, we don't need parameter completion
    // This function is kept for compatibility but returns empty
    vec![]
}

/// Execute a command, returning a stream and output shape (or None for Exit/Clear)
pub async fn execute(
    cmd: Command,
    bundle: &Arc<dyn BundleFacade>,
) -> ReplCommandResult {
    match cmd {
        Command::Sql(sql_str) => {
            let (stream, shape) = sql::execute(bundle, &sql_str).await?;
            Ok(Some((stream, shape)))
        }
        Command::Repl(repl_cmd) => repl_cmd.execute(bundle).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_attach() {
        let cmd = parse("ATTACH 'data.parquet'").unwrap();
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
        let cmd = parse("FILTER WHERE country = 'USA'").unwrap();
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
            parse("/help").unwrap(),
            Command::Repl(ReplCommand::Help)
        ));
        assert!(matches!(
            parse("/exit").unwrap(),
            Command::Repl(ReplCommand::Exit)
        ));
        assert!(matches!(
            parse("/quit").unwrap(),
            Command::Repl(ReplCommand::Exit)
        ));
        assert!(matches!(
            parse("/schema").unwrap(),
            Command::Repl(ReplCommand::Schema)
        ));
        assert!(matches!(
            parse("/count").unwrap(),
            Command::Repl(ReplCommand::Count)
        ));
        assert!(matches!(
            parse("/details").unwrap(),
            Command::Repl(ReplCommand::Details)
        ));
        assert!(matches!(
            parse("/info").unwrap(),
            Command::Repl(ReplCommand::Details)
        ));
        assert!(matches!(
            parse("/history").unwrap(),
            Command::Repl(ReplCommand::History)
        ));
        assert!(matches!(
            parse("/status").unwrap(),
            Command::Repl(ReplCommand::Status)
        ));
        assert!(matches!(
            parse("/clear").unwrap(),
            Command::Repl(ReplCommand::Clear)
        ));
    }

    #[test]
    fn test_parse_repl_commands_case_insensitive() {
        assert!(matches!(
            parse("/HELP").unwrap(),
            Command::Repl(ReplCommand::Help)
        ));
        assert!(matches!(
            parse("/Help").unwrap(),
            Command::Repl(ReplCommand::Help)
        ));
        assert!(matches!(
            parse("/EXIT").unwrap(),
            Command::Repl(ReplCommand::Exit)
        ));
        assert!(matches!(
            parse("/SCHEMA").unwrap(),
            Command::Repl(ReplCommand::Schema)
        ));
        assert!(matches!(
            parse("/COUNT").unwrap(),
            Command::Repl(ReplCommand::Count)
        ));
    }

    #[test]
    fn test_parse_repl_commands_with_space_after_slash() {
        assert!(matches!(
            parse("/ help").unwrap(),
            Command::Repl(ReplCommand::Help)
        ));
        assert!(matches!(
            parse("/  schema").unwrap(),
            Command::Repl(ReplCommand::Schema)
        ));
    }

    #[test]
    fn test_bare_repl_command_errors_with_suggestion() {
        let result = parse("HELP");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("/help"), "Error should suggest /help: {}", err);

        let result = parse("SHOW");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("/show"), "Error should suggest /show: {}", err);
    }

    #[test]
    fn test_unknown_repl_command() {
        let result = parse("/foo");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("Unknown command: /foo"),
            "Error: {}",
            err
        );
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
        let cmd = parse("COMMIT 'my commit message'").unwrap();
        match cmd {
            Command::Sql(sql) => {
                assert!(sql.contains("COMMIT"));
                assert!(sql.contains("my commit message"));
            }
            _ => panic!("Expected Sql command"),
        }
    }

    #[test]
    fn test_parse_show() {
        let cmd = parse("/show").unwrap();
        match cmd {
            Command::Repl(ReplCommand::Show { limit }) => assert_eq!(limit, None),
            _ => panic!("Expected Show command"),
        }

        let cmd = parse("/show limit 20").unwrap();
        match cmd {
            Command::Repl(ReplCommand::Show { limit }) => assert_eq!(limit, Some(20)),
            _ => panic!("Expected Show command"),
        }

        let cmd = parse("/SHOW LIMIT 20").unwrap();
        match cmd {
            Command::Repl(ReplCommand::Show { limit }) => assert_eq!(limit, Some(20)),
            _ => panic!("Expected Show command"),
        }
    }
}
