use crate::state::State;
use bundlebase::{
    bundle::{parse_command, BundleFacade},
    BundlebaseError,
};
use std::fmt::Display;
use std::sync::Arc;
use bundlebase::bundle::BundleCommand;

#[derive(Debug, Clone)]
pub enum Command {
    // SQL operations (delegated to BundleCommand)
    Sql(BundleCommand),

    // REPL-only commands (not SQL)
    Show {
        limit: Option<usize>,
    },
    Schema,
    Count,
    Explain,
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
    List(Vec<Box<dyn Display>>),
    None,
}

/// Parse input string into Command using SQL syntax
pub fn parse(input: &str) -> Result<Command, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Empty command".to_string());
    }

    let upper = input.to_uppercase();

    // Handle REPL-specific commands (not SQL)
    if upper == "HELP" {
        return Ok(Command::Help);
    } else if upper == "EXIT" || upper == "QUIT" {
        return Ok(Command::Exit);
    } else if upper == "CLEAR" {
        return Ok(Command::Clear);
    } else if upper == "SCHEMA" {
        return Ok(Command::Schema);
    } else if upper == "COUNT" {
        return Ok(Command::Count);
    } else if upper == "EXPLAIN" {
        return Ok(Command::Explain);
    } else if upper == "HISTORY" {
        return Ok(Command::History);
    } else if upper == "STATUS" {
        return Ok(Command::Status);
    } else if upper.starts_with("SHOW") {
        // Parse: SHOW [LIMIT <n>]
        let limit = if let Some(limit_str) = upper
            .strip_prefix("SHOW")
            .and_then(|s| s.trim().strip_prefix("LIMIT"))
        {
            limit_str.trim().parse().ok()
        } else {
            None
        };
        return Ok(Command::Show { limit });
    }

    // Handle bundle lifecycle commands (BundleCommand but with special REPL parsing)
    if upper == "RESET" {
        return Ok(Command::Sql(BundleCommand::Reset));
    } else if upper == "UNDO" {
        return Ok(Command::Sql(BundleCommand::Undo));
    } else if upper.starts_with("COMMIT") {
        // Parse: COMMIT '<message>'
        let message = input
            .strip_prefix("COMMIT")
            .ok_or("Invalid COMMIT syntax")?
            .trim()
            .trim_matches(|c| c == '\'' || c == '"')
            .to_string();
        if message.is_empty() {
            return Err("COMMIT requires a message".to_string());
        }
        return Ok(Command::Sql(BundleCommand::Commit { message }));
    }

    // Everything else is SQL - parse and wrap
    let sql_cmd = parse_command(input).map_err(|e| format!("Invalid SQL: {}", e))?;

    Ok(Command::Sql(sql_cmd))
}


/// Execute a command
pub async fn execute(cmd: Command, state: &Arc<State>) -> Result<ExecuteResult, BundlebaseError> {
    use crate::repl::display;

    match cmd {
        // SQL operations - delegate to BundleCommand
        Command::Sql(sql_cmd) => {
            sql_cmd.execute(&mut state.bundle.write()).await?;
            Ok(ExecuteResult::None)
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
        Command::Explain => {
            let plan = state.bundle.read().bundle.explain().await?;
            Ok(ExecuteResult::Message(plan))
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
  SHOW [LIMIT <n>]                     Display rows (default: 10)

Query & Transform:
  SELECT col1, col2, ... FROM data     Select columns (supports full SQL)
  FILTER WHERE <condition>             Filter rows by condition
  ALTER TABLE data DROP COLUMN <col>   Remove column
  ALTER TABLE data RENAME COLUMN <old> TO <new>  Rename column

Join Data:
  [LEFT|RIGHT|FULL|INNER] JOIN AS <name> ON <expression>
    Example: LEFT JOIN AS users ON data.user_id = users.id

Indexing:
  CREATE INDEX ON data(<column>)       Create index on column
  REINDEX                              Rebuild all indexes

Persistence:
  COMMIT '<message>'                   Commit changes with message
  RESET                                Discard all uncommitted changes
  UNDO                                 Undo the last operation

Schema & Info:
  SCHEMA                               Show table schema
  COUNT                                Show row count
  EXPLAIN                              Show query plan
  HISTORY                              Show commit history
  STATUS                               Show uncommitted changes

Meta Commands:
  HELP                                 Show this help
  EXIT, QUIT                           Exit REPL
  CLEAR                                Clear screen

Examples:
  ATTACH 'users.parquet'
  FILTER WHERE age > 21 AND country = 'USA'
  SELECT name, email, salary * 1.1 AS new_salary FROM data
  LEFT JOIN AS departments ON data.dept_id = departments.id
  CREATE INDEX ON data(email)
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
            Command::Sql(BundleCommand::Attach { path }) => assert_eq!(path, "data.parquet"),
            _ => panic!("Expected Sql(Attach) command"),
        }
    }

    #[test]
    fn test_parse_filter() {
        let cmd = parse("FILTER WHERE country = 'USA'").unwrap();
        match cmd {
            Command::Sql(BundleCommand::Filter { where_clause, .. }) => {
                assert_eq!(where_clause, "country = 'USA'")
            }
            _ => panic!("Expected Sql(Filter) command"),
        }
    }

    #[test]
    fn test_parse_select() {
        let cmd = parse("SELECT name, email FROM data").unwrap();
        match cmd {
            Command::Sql(BundleCommand::Select { columns }) => {
                assert_eq!(columns.len(), 2);
                assert!(columns.contains(&"name".to_string()));
                assert!(columns.contains(&"email".to_string()));
            }
            _ => panic!("Expected Sql(Select) command"),
        }
    }

    #[test]
    fn test_parse_meta_commands() {
        assert!(matches!(parse("HELP").unwrap(), Command::Help));
        assert!(matches!(parse("EXIT").unwrap(), Command::Exit));
        assert!(matches!(parse("SCHEMA").unwrap(), Command::Schema));
        assert!(matches!(parse("COUNT").unwrap(), Command::Count));
    }

    #[test]
    fn test_parse_commit() {
        let cmd = parse("COMMIT 'my commit message'").unwrap();
        match cmd {
            Command::Sql(BundleCommand::Commit { message }) => {
                assert_eq!(message, "my commit message")
            }
            _ => panic!("Expected Sql(Commit) command"),
        }
    }

    #[test]
    fn test_parse_show() {
        let cmd = parse("SHOW LIMIT 20").unwrap();
        match cmd {
            Command::Show { limit } => assert_eq!(limit, Some(20)),
            _ => panic!("Expected Show command"),
        }
    }
}
