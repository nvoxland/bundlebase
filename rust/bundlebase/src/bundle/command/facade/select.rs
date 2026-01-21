//! Select command implementation.
//!
//! SelectCommand is a facade command - it works with `BundleFacade::select()` to
//! produce a new BundleBuilder. It does not mutate the source bundle.

use crate::bundle::command::{CommandParsing, Rule};
use crate::BundlebaseError;
use datafusion::scalar::ScalarValue;

/// Command to execute a SQL SELECT query.
///
/// SelectCommand is executed via `BundleFacade.select()` which returns a new
/// BundleBuilder with the query applied. The source bundle is not modified.
///
/// When executed via `BundleCommand.execute()`, the builder is replaced with
/// the result of the select operation.
#[derive(Debug, Clone)]
pub struct SelectCommand {
    /// The SQL query
    pub sql: String,
    /// Parameters for the query ($1, $2, etc.)
    pub params: Vec<ScalarValue>,
}

impl SelectCommand {
    /// Create a new SelectCommand.
    pub fn new(sql: impl Into<String>, params: Vec<ScalarValue>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }
}

impl CommandParsing for SelectCommand {
    fn rule() -> Rule {
        Rule::select_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        // Capture the full SELECT statement as raw SQL
        let raw = pair.as_str().to_string();
        Ok(SelectCommand::new(raw, vec![]))
    }

    fn to_statement(&self) -> String {
        self.sql.clone()
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_select() {
        let input = "SELECT * FROM bundle";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Select(c) => {
                assert!(c.sql.to_uppercase().contains("SELECT"));
                assert!(c.sql.contains("bundle"));
            }
            _ => panic!("Expected Select variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = SelectCommand::new("SELECT name, email FROM bundle WHERE id > 10", vec![]);
        let statement = cmd.to_statement();

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Select(c) => {
                assert!(c.sql.contains("name"));
                assert!(c.sql.contains("email"));
            }
            _ => panic!("Expected Select variant"),
        }
    }
}
