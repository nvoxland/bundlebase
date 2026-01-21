use crate::state::State;
use bundlebase::{
    bundle::{parse_command, BundleCommand, BundleFacade, CommandOutput},
    source::format_fetch_summary,
    BundlebaseError,
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Command {
    // SQL operations (delegated to BundleCommand)
    Sql(BundleCommand),

    // REPL-only commands (not SQL)
    Show { limit: Option<usize> },
    Schema,
    Count,
    History,
    Status,

    // Meta commands
    Help,
    Exit,
    Clear,
}

pub enum ExecuteResult {
    Message(String),
    Table(String),
    None,
}

/// Parse input string into Command using SQL syntax
/// Meta commands start with `/` (e.g., `/help`, `/show`)
/// All other input is treated as SQL
pub fn parse(input: &str) -> Result<Command, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Empty command".to_string());
    }

    // Check for meta command (starts with /)
    if input.starts_with('/') {
        let meta_input = input[1..].trim();
        if meta_input.is_empty() {
            return Err("Empty meta command after '/'. Type /help for available commands.".to_string());
        }
        return parse_meta_command(meta_input);
    }

    // Everything else is SQL - parse and wrap
    let sql_cmd = parse_command(input).map_err(|e| {
        // Check if the user might have meant a meta command
        if let Some(suggestion) = suggest_meta_command(input) {
            format!("Invalid SQL: {}. Did you mean '{}'?", e, suggestion)
        } else {
            format!("Invalid SQL: {}", e)
        }
    })?;

    Ok(Command::Sql(sql_cmd))
}

/// Parse a meta command (input without the leading `/`)
fn parse_meta_command(input: &str) -> Result<Command, String> {
    let upper = input.to_uppercase();

    // Handle single-word meta commands
    match upper.as_str() {
        "HELP" => return Ok(Command::Help),
        "EXIT" | "QUIT" => return Ok(Command::Exit),
        "CLEAR" => return Ok(Command::Clear),
        "SCHEMA" => return Ok(Command::Schema),
        "COUNT" => return Ok(Command::Count),
        "HISTORY" => return Ok(Command::History),
        "STATUS" => return Ok(Command::Status),
        _ => {}
    }

    // Handle SHOW with optional LIMIT
    if upper.starts_with("SHOW") {
        let limit = upper
            .strip_prefix("SHOW")
            .and_then(|s| s.trim().strip_prefix("LIMIT"))
            .and_then(|s| s.trim().parse().ok());
        return Ok(Command::Show { limit });
    }

    Err(format!("Unknown meta command: /{}. Type /help for available commands.", input))
}

/// Check if input looks like a bare meta command and suggest the `/` prefix
fn suggest_meta_command(input: &str) -> Option<String> {
    let upper = input.to_uppercase();
    let first_word = upper.split_whitespace().next().unwrap_or("");

    match first_word {
        "HELP" => Some("/help".to_string()),
        "EXIT" => Some("/exit".to_string()),
        "QUIT" => Some("/quit".to_string()),
        "CLEAR" => Some("/clear".to_string()),
        "SCHEMA" => Some("/schema".to_string()),
        "COUNT" => Some("/count".to_string()),
        "HISTORY" => Some("/history".to_string()),
        "STATUS" => Some("/status".to_string()),
        "SHOW" => Some("/show".to_string()),
        _ => None,
    }
}

/// Execute a command
pub async fn execute(cmd: Command, state: &Arc<State>) -> Result<ExecuteResult, BundlebaseError> {
    use crate::repl::display;

    match cmd {
        // SQL operations - delegate to BundleCommand
        Command::Sql(sql_cmd) => {
            let output = sql_cmd.execute(&mut state.bundle.write()).await?;
            match output {
                CommandOutput::Empty => Ok(ExecuteResult::None),
                CommandOutput::Verification(results) => {
                    Ok(ExecuteResult::Message(results.to_string()))
                }
                CommandOutput::Fetch(results) => {
                    Ok(ExecuteResult::Message(format_fetch_summary(&results)))
                }
                CommandOutput::ExplainPlan(plan) => Ok(ExecuteResult::Message(plan)),
            }
        }

        // REPL-only commands
        Command::Show { limit } => {
            let df = state.bundle.read().dataframe().await?;
            let table = display::display_dataframe(&df, limit).await?;
            Ok(ExecuteResult::Table(table))
        }
        Command::Schema => {
            let schema = state.bundle.read().schema().await?;
            let table = display::display_schema(schema);
            Ok(ExecuteResult::Table(table))
        }
        Command::Count => {
            let count = state.bundle.read().num_rows().await?;
            Ok(ExecuteResult::Message(format!("Row count: {}", count)))
        }
        Command::History => {
            let commits = state.bundle.read().history();
            let table = display::display_history(commits);
            Ok(ExecuteResult::Table(table))
        }
        Command::Status => {
            let guard = state.bundle.read();
            let status = guard.status();
            Ok(ExecuteResult::Message(status.to_string()))
        }
        Command::Help => {
            let help_text = r#"
Bundlebase REPL - SQL Interface

Data Operations:
  ATTACH '<path>'                      Attach data source
  /show [limit <n>]                    Display rows (default: 10)

Query & Transform:
  SELECT col1, col2, ... FROM bundle     Select columns (supports full SQL)
  FILTER WHERE <condition>             Filter rows by condition
  ALTER TABLE bundle DROP COLUMN <col>   Remove column
  ALTER TABLE bundle RENAME COLUMN <old> TO <new>  Rename column

Join Data:
  [LEFT|RIGHT|FULL|INNER] JOIN AS <name> ON <expression>
    Example: LEFT JOIN AS users ON bundle.user_id = users.id

Indexing:
  CREATE INDEX ON bundle(<column>)       Create index on column
  REINDEX                              Rebuild all indexes

Data Integrity:
  VERIFY DATA                          Verify all data file hashes
  VERIFY DATA UPDATE                   Verify and update version strings

Persistence:
  COMMIT '<message>'                   Commit changes with message
  RESET                                Discard all uncommitted changes
  UNDO                                 Undo the last operation

Schema & Info:
  /schema                              Show table schema
  /count                               Show row count
  EXPLAIN PLAN                         Show query plan
  /history                             Show commit history
  /status                              Show uncommitted changes

Meta Commands:
  /help                                Show this help
  /exit, /quit                         Exit REPL
  /clear                               Clear screen

Examples:
  ATTACH 'users.parquet'
  FILTER WHERE age > 21 AND country = 'USA'
  SELECT name, email, salary * 1.1 AS new_salary FROM bundle
  LEFT JOIN AS departments ON bundle.dept_id = departments.id
  CREATE INDEX ON bundle(email)
  COMMIT 'Added filtering and joined departments'
"#;
            Ok(ExecuteResult::Message(help_text.to_string()))
        }
        Command::Clear => {
            print!("\x1B[2J\x1B[1;1H");
            Ok(ExecuteResult::None)
        }
        Command::Exit => Ok(ExecuteResult::None),
    }
}

/// Get SQL command suggestions (for tab completion)
pub fn get_parameter_names(_command_name: &str) -> Vec<String> {
    // With SQL syntax, we don't need parameter completion
    // This function is kept for compatibility but returns empty
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_attach() {
        let cmd = parse("ATTACH 'data.parquet'").unwrap();
        match cmd {
            Command::Sql(BundleCommand::Attach(attach_cmd)) => {
                assert_eq!(attach_cmd.path, "data.parquet");
                assert_eq!(attach_cmd.pack, None);
            }
            _ => panic!("Expected Sql(Attach) command"),
        }
    }

    #[test]
    fn test_parse_filter() {
        let cmd = parse("FILTER WHERE country = 'USA'").unwrap();
        match cmd {
            Command::Sql(BundleCommand::Filter(filter_cmd)) => {
                assert_eq!(filter_cmd.where_clause, "country = 'USA'")
            }
            _ => panic!("Expected Sql(Filter) command"),
        }
    }

    #[test]
    fn test_parse_meta_commands() {
        // Meta commands require / prefix
        assert!(matches!(parse("/help").unwrap(), Command::Help));
        assert!(matches!(parse("/exit").unwrap(), Command::Exit));
        assert!(matches!(parse("/quit").unwrap(), Command::Exit));
        assert!(matches!(parse("/schema").unwrap(), Command::Schema));
        assert!(matches!(parse("/count").unwrap(), Command::Count));
        assert!(matches!(parse("/history").unwrap(), Command::History));
        assert!(matches!(parse("/status").unwrap(), Command::Status));
        assert!(matches!(parse("/clear").unwrap(), Command::Clear));
    }

    #[test]
    fn test_parse_meta_commands_case_insensitive() {
        // Meta commands are case insensitive
        assert!(matches!(parse("/HELP").unwrap(), Command::Help));
        assert!(matches!(parse("/Help").unwrap(), Command::Help));
        assert!(matches!(parse("/EXIT").unwrap(), Command::Exit));
        assert!(matches!(parse("/SCHEMA").unwrap(), Command::Schema));
        assert!(matches!(parse("/COUNT").unwrap(), Command::Count));
    }

    #[test]
    fn test_parse_meta_commands_with_space_after_slash() {
        // Space after / should work
        assert!(matches!(parse("/ help").unwrap(), Command::Help));
        assert!(matches!(parse("/  schema").unwrap(), Command::Schema));
    }

    #[test]
    fn test_bare_meta_command_errors_with_suggestion() {
        // Bare meta commands (without /) should fail with suggestion
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
    fn test_unknown_meta_command() {
        let result = parse("/foo");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Unknown meta command: /foo"), "Error: {}", err);
        assert!(err.contains("/help"), "Error should suggest /help: {}", err);
    }

    #[test]
    fn test_empty_meta_command() {
        let result = parse("/");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Empty meta command"), "Error: {}", err);
    }

    #[test]
    fn test_parse_commit() {
        let cmd = parse("COMMIT 'my commit message'").unwrap();
        match cmd {
            Command::Sql(BundleCommand::Commit(commit_cmd)) => {
                assert_eq!(commit_cmd.message, "my commit message")
            }
            _ => panic!("Expected Sql(Commit) command"),
        }
    }

    #[test]
    fn test_parse_show() {
        // Show requires / prefix
        let cmd = parse("/show").unwrap();
        match cmd {
            Command::Show { limit } => assert_eq!(limit, None),
            _ => panic!("Expected Show command"),
        }

        let cmd = parse("/show limit 20").unwrap();
        match cmd {
            Command::Show { limit } => assert_eq!(limit, Some(20)),
            _ => panic!("Expected Show command"),
        }

        let cmd = parse("/SHOW LIMIT 20").unwrap();
        match cmd {
            Command::Show { limit } => assert_eq!(limit, Some(20)),
            _ => panic!("Expected Show command"),
        }
    }
}
