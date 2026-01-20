//! Select command implementation.

use crate::bundle::command::{Command, CommandContext, Rule};
use crate::bundle::operation::SelectOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use datafusion::scalar::ScalarValue;
use log::info;

/// Command to execute a SQL SELECT query.
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

#[async_trait]
impl Command for SelectCommand {
    type Output = ();

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let sql = if !self.sql.to_lowercase().starts_with("select ") {
            format!("SELECT {}", self.sql)
        } else {
            self.sql
        };

        ctx.apply_operation(SelectOp::setup(sql, self.params).await?.into())
            .await?;
        info!("Created query");
        Ok(())
    }

    fn rule() -> Option<Rule> {
        Some(Rule::select_stmt)
    }

    fn from_pest(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
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
