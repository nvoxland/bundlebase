//! Syntax command implementation.
//!
//! Returns syntax and usage information for bundlebase commands.
//! With no argument, lists all available commands.
//! With a command name, shows detailed syntax and examples.

use crate::response::OutputShape;
use crate::{BundleCommand, BundleFacadeCommand, CommandParsing, CommandResponse, Rule};
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use arrow::datatypes::SchemaRef;

/// Command to show syntax and usage for bundlebase commands.
#[derive(Debug, Clone)]
pub struct SyntaxCommand {
    /// Optional command name to look up (e.g., "IMPORT CONNECTOR").
    /// If None, lists all available commands.
    pub query: Option<String>,
}

impl SyntaxCommand {
    /// Returns the Arrow schema for syntax output.
    pub fn output_schema() -> SchemaRef {
        String::schema()
    }

    /// Returns the expected output shape.
    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }

    /// Look up syntax for a specific command, or list all commands.
    fn lookup(&self) -> Result<String, BundlebaseError> {
        let commands = BundleCommand::available_commands();

        match &self.query {
            None => {
                // List all commands sorted by name
                let mut entries: Vec<_> = commands.into_iter().collect();
                entries.sort_by_key(|(name, _)| name.to_string());

                let mut output = String::from("Available commands:\n\n");
                for (name, syntax) in &entries {
                    output.push_str(&format!("  {:<25} {}\n", name, syntax));
                }
                output.push_str("\nUse SYNTAX <command> for detailed usage and examples.");
                Ok(output)
            }
            Some(query) => {
                let query_upper = query.trim().to_uppercase();

                // Find matching command (exact match first, then prefix match)
                let matched = commands
                    .iter()
                    .find(|(name, _)| **name == query_upper)
                    .or_else(|| {
                        commands
                            .iter()
                            .find(|(name, _)| name.starts_with(&query_upper))
                    });

                match matched {
                    Some((name, syntax)) => {
                        let mut output = format!("{}\n\nSyntax:\n  {}\n", name, syntax);

                        // Look up the markdown usage file
                        if let Some(usage) = usage_for_command(name) {
                            output.push('\n');
                            output.push_str(usage);
                        }

                        Ok(output)
                    }
                    None => Err(format!(
                        "Unknown command: '{}'. Use SYNTAX to list all available commands.",
                        query.trim()
                    )
                    .into()),
                }
            }
        }
    }
}

impl CommandParsing for SyntaxCommand {
    fn rule() -> Rule {
        Rule::syntax_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut query = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::syntax_query {
                query = Some(inner_pair.as_str().to_string());
            }
        }

        Ok(SyntaxCommand { query })
    }

    fn to_statement(&self) -> String {
        match &self.query {
            Some(q) => format!("SYNTAX {}", q),
            None => "SYNTAX".to_string(),
        }
    }
}

impl BundleFacadeCommand for SyntaxCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        _facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        self.lookup()
    }
}

/// Returns the embedded markdown usage text for a command, if available.
fn usage_for_command(command_name: &str) -> Option<&'static str> {
    match command_name {
        "ATTACH" => Some(include_str!("../syntax/attach.md")),
        "DETACH" => Some(include_str!("../syntax/detach.md")),
        "FILTER" => Some(include_str!("../syntax/filter.md")),
        "JOIN" => Some(include_str!("../syntax/join.md")),
        "REPLACE" => Some(include_str!("../syntax/replace.md")),
        "ADD COLUMN" => Some(include_str!("../syntax/add_column.md")),
        "CAST COLUMN" => Some(include_str!("../syntax/cast_column.md")),
        "DROP COLUMN" => Some(include_str!("../syntax/drop_column.md")),
        "RENAME COLUMN" => Some(include_str!("../syntax/rename_column.md")),
        "CREATE INDEX" => Some(include_str!("../syntax/create_index.md")),
        "DROP INDEX" => Some(include_str!("../syntax/drop_index.md")),
        "REBUILD INDEX" => Some(include_str!("../syntax/rebuild_index.md")),
        "REINDEX" => Some(include_str!("../syntax/reindex.md")),
        "CREATE VIEW" => Some(include_str!("../syntax/create_view.md")),
        "DROP VIEW" => Some(include_str!("../syntax/drop_view.md")),
        "RENAME VIEW" => Some(include_str!("../syntax/rename_view.md")),
        "DROP JOIN" => Some(include_str!("../syntax/drop_join.md")),
        "RENAME JOIN" => Some(include_str!("../syntax/rename_join.md")),
        "SET NAME" => Some(include_str!("../syntax/set_name.md")),
        "SET DESCRIPTION" => Some(include_str!("../syntax/set_description.md")),
        "SAVE CONFIG" => Some(include_str!("../syntax/save_config.md")),
        "SET CONFIG" => Some(include_str!("../syntax/set_config.md")),
        "IMPORT CONNECTOR" => Some(include_str!("../syntax/import_connector.md")),
        "IMPORT FUNCTION" => Some(include_str!("../syntax/import_function.md")),
        "IMPORT TEMP CONNECTOR" => Some(include_str!("../syntax/import_temp_connector.md")),
        "IMPORT TEMP FUNCTION" => Some(include_str!("../syntax/import_temp_function.md")),
        "RENAME CONNECTOR" => Some(include_str!("../syntax/rename_connector.md")),
        "RENAME FUNCTION" => Some(include_str!("../syntax/rename_function.md")),
        "RENAME TEMP CONNECTOR" => Some(include_str!("../syntax/rename_temp_connector.md")),
        "RENAME TEMP FUNCTION" => Some(include_str!("../syntax/rename_temp_function.md")),
        "DROP CONNECTOR" => Some(include_str!("../syntax/drop_connector.md")),
        "DROP FUNCTION" => Some(include_str!("../syntax/drop_function.md")),
        "DROP TEMP CONNECTOR" => Some(include_str!("../syntax/drop_temp_connector.md")),
        "DROP TEMP FUNCTION" => Some(include_str!("../syntax/drop_temp_function.md")),
        "CREATE SOURCE" => Some(include_str!("../syntax/create_source.md")),
        "FETCH" => Some(include_str!("../syntax/fetch.md")),
        "FETCH ALL" => Some(include_str!("../syntax/fetch_all.md")),
        "COMMIT" => Some(include_str!("../syntax/commit.md")),
        "RESET" => Some(include_str!("../syntax/reset.md")),
        "UNDO" => Some(include_str!("../syntax/undo.md")),
        "VERIFY DATA" => Some(include_str!("../syntax/verify_data.md")),
        "EXPLAIN" => Some(include_str!("../syntax/explain.md")),
        "DESCRIBE CONNECTOR" => Some(include_str!("../syntax/describe_connector.md")),
        "DESCRIBE FUNCTION" => Some(include_str!("../syntax/describe_function.md")),
        "SHOW" => Some(include_str!("../syntax/show.md")),
        "SYNTAX" => Some(include_str!("../syntax/syntax.md")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_syntax_no_args() {
        let cmd = parse_command("SYNTAX").expect("Failed to parse SYNTAX");
        match cmd {
            BundleCommand::Syntax(s) => assert!(s.query.is_none()),
            _ => panic!("Expected Syntax variant"),
        }
    }

    #[test]
    fn test_parse_syntax_with_command() {
        let cmd = parse_command("SYNTAX ATTACH").expect("Failed to parse SYNTAX ATTACH");
        match cmd {
            BundleCommand::Syntax(s) => assert_eq!(s.query.as_deref(), Some("ATTACH")),
            _ => panic!("Expected Syntax variant"),
        }
    }

    #[test]
    fn test_parse_syntax_multi_word() {
        let cmd = parse_command("SYNTAX IMPORT CONNECTOR")
            .expect("Failed to parse SYNTAX IMPORT CONNECTOR");
        match cmd {
            BundleCommand::Syntax(s) => {
                assert_eq!(s.query.as_deref(), Some("IMPORT CONNECTOR"))
            }
            _ => panic!("Expected Syntax variant"),
        }
    }

    #[test]
    fn test_parse_syntax_case_insensitive() {
        let cmd = parse_command("syntax attach").expect("Failed to parse syntax attach");
        match cmd {
            BundleCommand::Syntax(s) => assert_eq!(s.query.as_deref(), Some("attach")),
            _ => panic!("Expected Syntax variant"),
        }
    }

    #[test]
    fn test_lookup_all_commands() {
        let cmd = SyntaxCommand { query: None };
        let result = cmd.lookup().expect("Should list all commands");
        assert!(result.contains("ATTACH"));
        assert!(result.contains("IMPORT CONNECTOR"));
        assert!(result.contains("COMMIT"));
    }

    #[test]
    fn test_lookup_specific_command() {
        let cmd = SyntaxCommand {
            query: Some("ATTACH".to_string()),
        };
        let result = cmd.lookup().expect("Should return ATTACH syntax");
        assert!(result.contains("ATTACH"));
        assert!(result.contains("Syntax:"));
        assert!(result.contains("Examples"));
    }

    #[test]
    fn test_lookup_case_insensitive() {
        let cmd = SyntaxCommand {
            query: Some("attach".to_string()),
        };
        let result = cmd.lookup().expect("Should return ATTACH syntax");
        assert!(result.contains("ATTACH"));
    }

    #[test]
    fn test_lookup_multi_word() {
        let cmd = SyntaxCommand {
            query: Some("IMPORT CONNECTOR".to_string()),
        };
        let result = cmd.lookup().expect("Should return IMPORT CONNECTOR syntax");
        assert!(result.contains("IMPORT CONNECTOR"));
    }

    #[test]
    fn test_lookup_unknown_command() {
        let cmd = SyntaxCommand {
            query: Some("NONSENSE".to_string()),
        };
        let result = cmd.lookup();
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip_no_args() {
        let cmd = SyntaxCommand { query: None };
        assert_eq!(cmd.to_statement(), "SYNTAX");
    }

    #[test]
    fn test_roundtrip_with_args() {
        let cmd = SyntaxCommand {
            query: Some("ATTACH".to_string()),
        };
        assert_eq!(cmd.to_statement(), "SYNTAX ATTACH");
    }
}
