use crate::state::State;
use bundlebase::{
    bundle::{BundleFacade, JoinTypeOption},
    BundlebaseError, Operation,
};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Command {
    // Data operations
    Attach {
        url: String,
    },
    Show {
        limit: Option<usize>,
    },

    // Transformations
    Filter {
        where_clause: String,
    },
    Select {
        columns: Vec<String>,
    },
    RemoveColumn {
        name: String,
    },
    RenameColumn {
        old: String,
        new: String,
    },
    Query {
        sql: String,
    },
    Join {
        name: String,
        url: String,
        expression: String,
        join_type: Option<String>,
    },

    // Schema & info
    Schema,
    Count,
    Explain,
    History,
    Status,

    // Indexing
    Index {
        column: String,
    },
    Reindex,

    // Persistence
    Commit {
        message: String,
    },
    Reset,
    undo,

    // Meta
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

/// Parse input string into Command
pub fn parse(input: &str) -> Result<Command, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Empty command".to_string());
    }

    // Split by whitespace, but respect quoted strings
    let parts = parse_tokens(input)?;
    if parts.is_empty() {
        return Err("Empty command".to_string());
    }

    let command_name = parts[0].as_str();
    let params = parse_parameters(&parts[1..])?;

    match command_name {
        "attach" => {
            let url = params
                .get("url")
                .ok_or("Missing 'url' parameter")?
                .to_string();
            Ok(Command::Attach { url })
        }
        "show" => {
            let limit = params.get("limit").and_then(|s| s.parse().ok());
            Ok(Command::Show { limit })
        }
        "filter" => {
            let where_clause = params
                .get("where")
                .ok_or("Missing 'where' parameter")?
                .to_string();
            Ok(Command::Filter { where_clause })
        }
        "select" => {
            let columns_str = params.get("columns").ok_or("Missing 'columns' parameter")?;
            let columns = columns_str
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            Ok(Command::Select { columns })
        }
        "remove" => {
            let name = params
                .get("name")
                .ok_or("Missing 'name' parameter")?
                .to_string();
            Ok(Command::RemoveColumn { name })
        }
        "rename" => {
            let old = params
                .get("old")
                .ok_or("Missing 'old' parameter")?
                .to_string();
            let new = params
                .get("new")
                .ok_or("Missing 'new' parameter")?
                .to_string();
            Ok(Command::RenameColumn { old, new })
        }
        "query" => {
            let sql = params
                .get("sql")
                .ok_or("Missing 'sql' parameter")?
                .to_string();
            Ok(Command::Query { sql })
        }
        "join" => {
            let name = params
                .get("name")
                .ok_or("Missing 'name' parameter")?
                .to_string();
            let url = params
                .get("url")
                .ok_or("Missing 'url' parameter")?
                .to_string();
            let expression = params
                .get("expression")
                .ok_or("Missing 'expression' parameter")?
                .to_string();
            let join_type = params.get("join_type").map(|s| s.to_string());
            Ok(Command::Join {
                name,
                url,
                expression,
                join_type,
            })
        }
        "schema" => Ok(Command::Schema),
        "count" => Ok(Command::Count),
        "explain" => Ok(Command::Explain),
        "history" => Ok(Command::History),
        "status" => Ok(Command::Status),
        "index" => {
            let column = params
                .get("column")
                .ok_or("Missing 'column' parameter")?
                .to_string();
            Ok(Command::Index { column })
        }
        "reindex" => Ok(Command::Reindex),
        "commit" => {
            let message = params
                .get("message")
                .ok_or("Missing 'message' parameter")?
                .to_string();
            Ok(Command::Commit { message })
        }
        "reset" => Ok(Command::Reset),
        "undo" => Ok(Command::undo),
        "help" => Ok(Command::Help),
        "exit" | "quit" => Ok(Command::Exit),
        "clear" => Ok(Command::Clear),
        _ => Err(format!("Unknown command: {}", command_name)),
    }
}

/// Parse key=value parameters
fn parse_parameters(tokens: &[String]) -> Result<HashMap<String, String>, String> {
    let mut params = HashMap::new();

    for token in tokens {
        if let Some((key, value)) = token.split_once('=') {
            params.insert(key.trim().to_string(), value.trim().to_string());
        } else {
            return Err(format!(
                "Invalid parameter format: '{}'. Expected key=value",
                token
            ));
        }
    }

    Ok(params)
}

/// Parse tokens from input, respecting quoted strings
fn parse_tokens(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current_token = String::new();
    let mut in_quotes = false;
    let mut quote_char = ' ';
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' | '\'' if !in_quotes => {
                in_quotes = true;
                quote_char = ch;
            }
            '"' | '\'' if in_quotes && ch == quote_char => {
                in_quotes = false;
                quote_char = ' ';
            }
            ' ' | '\t' if !in_quotes => {
                if !current_token.is_empty() {
                    tokens.push(current_token.clone());
                    current_token.clear();
                }
            }
            _ => {
                current_token.push(ch);
            }
        }
    }

    if in_quotes {
        return Err(format!("Unclosed quote ({})", quote_char));
    }

    if !current_token.is_empty() {
        tokens.push(current_token);
    }

    Ok(tokens)
}

/// Execute a command
pub async fn execute(cmd: Command, state: &Arc<State>) -> Result<ExecuteResult, BundlebaseError> {
    use crate::repl::display;

    match cmd {
        Command::Attach { url } => {
            state.bundle.write().attach(&url).await?;
            Ok(ExecuteResult::None)
        }
        Command::Show { limit } => {
            let df = state.bundle.read().dataframe().await?;
            let table = display::display_dataframe(&df, limit).await?;
            Ok(ExecuteResult::Table(table))
        }
        Command::Filter { where_clause } => {
            state
                .bundle
                .write()
                .filter(&where_clause, vec![])
                .await?;
            Ok(ExecuteResult::None)
        }
        Command::Select { columns } => {
            let col_refs: Vec<&str> = columns.iter().map(|s| s.as_str()).collect();
            state.bundle.write().select(col_refs).await?;
            Ok(ExecuteResult::None)
        }
        Command::RemoveColumn { name } => {
            state.bundle.write().remove_column(&name).await?;
            Ok(ExecuteResult::None)
        }
        Command::RenameColumn { old, new } => {
            state.bundle.write().rename_column(&old, &new).await?;
            Ok(ExecuteResult::None)
        }
        Command::Query { sql } => {
            state.bundle.write().query(&sql, vec![]).await?;
            Ok(ExecuteResult::None)
        }
        Command::Join {
            name,
            url,
            expression,
            join_type,
        } => {
            let join_type_opt = match join_type.as_deref() {
                Some("Left") => JoinTypeOption::Left,
                Some("Right") => JoinTypeOption::Right,
                Some("Full") => JoinTypeOption::Full,
                Some("Inner") | None => JoinTypeOption::Inner,
                Some(other) => return Err(format!("Invalid join type: {}", other).into()),
            };
            state
                .bundle
                .write()
                .join(&name, &url, &expression, join_type_opt)
                .await?;
            Ok(ExecuteResult::None)
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
        Command::Index { column } => {
            state.bundle.write().index(&column).await?;
            Ok(ExecuteResult::None)
        }
        Command::Reindex => {
            state.bundle.write().reindex().await?;
            Ok(ExecuteResult::None)
        }
        Command::Commit { message } => {
            state.bundle.write().commit(&message).await?;
            Ok(ExecuteResult::None)
        }
        Command::Reset => {
            state.bundle.write().reset().await?;
            Ok(ExecuteResult::None)
        }
        Command::undo => {
            state.bundle.write().undo().await?;
            Ok(ExecuteResult::None)
        }
        Command::Help => {
            let help_text = r#"
Bundlebase REPL Commands:

Data Operations:
  attach url=<path>           Attach data source
  show [limit=<n>]            Display rows (default: 10)

Transformations:
  filter where=<clause>       Filter rows
  select columns=<col1>,<col2>,...  Select columns
  remove name=<column>        Remove column
  rename old=<old> new=<new>  Rename column
  query sql=<sql>             Execute SQL query
  join name=<name> url=<url> expression=<expr> [join_type=<type>]

Schema & Info:
  schema                      Show schema
  count                       Show row count
  explain                     Show query plan
  history                     Show commit history
  status                      Show uncommitted changes

Indexing:
  index column=<column>       Create index
  drop-index column=<column>  Drop index
  reindex                     Rebuild all indexes

Persistence:
  commit message=<message>    Commit changes
  reset                       Discard all uncommitted changes
  undo, undo             Undo the last operation

Meta:
  help                        Show this help
  exit, quit                  Exit REPL
  clear                       Clear screen
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

/// Get valid parameter names for a command (for tab completion)
pub fn get_parameter_names(command_name: &str) -> Vec<String> {
    match command_name {
        "attach" => vec!["url".to_string()],
        "show" => vec!["limit".to_string()],
        "filter" => vec!["where".to_string()],
        "select" => vec!["columns".to_string()],
        "remove" => vec!["name".to_string()],
        "rename" => vec!["old".to_string(), "new".to_string()],
        "query" => vec!["sql".to_string()],
        "join" => vec![
            "name".to_string(),
            "url".to_string(),
            "expression".to_string(),
            "join_type".to_string(),
        ],
        "index" => vec!["column".to_string()],
        "drop-index" => vec!["column".to_string()],
        "commit" => vec!["message".to_string()],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_attach() {
        let cmd = parse("attach url=data.parquet").unwrap();
        match cmd {
            Command::Attach { url } => assert_eq!(url, "data.parquet"),
            _ => panic!("Expected Attach command"),
        }
    }

    #[test]
    fn test_parse_filter_with_quotes() {
        let cmd = parse("filter where=\"Country = 'USA'\"").unwrap();
        match cmd {
            Command::Filter { where_clause } => assert_eq!(where_clause, "Country = 'USA'"),
            _ => panic!("Expected Filter command"),
        }
    }

    #[test]
    fn test_parse_join() {
        let cmd = parse(
            "join name=regions url=data.csv expression='$base.id = regions.id' join_type=Inner",
        )
        .unwrap();
        match cmd {
            Command::Join {
                name,
                url,
                expression,
                join_type,
            } => {
                assert_eq!(name, "regions");
                assert_eq!(url, "data.csv");
                assert_eq!(expression, "$base.id = regions.id");
                assert_eq!(join_type, Some("Inner".to_string()));
            }
            _ => panic!("Expected Join command"),
        }
    }
}
