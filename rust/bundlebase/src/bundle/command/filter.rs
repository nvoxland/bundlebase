//! Filter command implementation.

use crate::bundle::command::{Command, CommandContext, Rule};
use crate::bundle::operation::FilterOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use datafusion::scalar::ScalarValue;
use log::info;

/// Command to filter rows with a WHERE clause.
#[derive(Debug, Clone)]
pub struct FilterCommand {
    /// The WHERE clause
    pub where_clause: String,
    /// Parameters for the WHERE clause ($1, $2, etc.)
    pub params: Vec<ScalarValue>,
}

impl FilterCommand {
    /// Create a new FilterCommand.
    pub fn new(where_clause: impl Into<String>, params: Vec<ScalarValue>) -> Self {
        Self {
            where_clause: where_clause.into(),
            params,
        }
    }
}

#[async_trait]
impl Command for FilterCommand {
    type Output = ();

    async fn execute(self: Box<Self>, ctx: &mut CommandContext<'_>) -> Result<(), BundlebaseError> {
        let statement = self.to_statement();
        ctx.apply_operation(
            FilterOp::setup(&self.where_clause, self.params)
                .await?
                .into(),
        )
        .await?;
        info!("Filtered: {}", statement);
        Ok(())
    }

    fn rule() -> Option<Rule> {
        Some(Rule::filter_stmt)
    }

    fn from_pest(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut where_clause = None;

        for inner_pair in pair.into_inner() {
            if let Rule::where_condition = inner_pair.as_rule() {
                where_clause = Some(inner_pair.as_str().trim().to_string());
            }
        }

        let where_clause = where_clause.ok_or_else(|| -> BundlebaseError {
            "FILTER statement missing WHERE clause".into()
        })?;

        if where_clause.is_empty() {
            return Err("FILTER WHERE clause cannot be empty".into());
        }

        Ok(FilterCommand::new(where_clause, vec![]))
    }

    fn to_statement(&self) -> String {
        format!("FILTER WHERE {}", self.where_clause)
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_filter_simple() {
        let input = "FILTER WHERE country = 'USA'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Filter(c) => {
                assert_eq!(c.where_clause, "country = 'USA'");
            }
            _ => panic!("Expected Filter variant"),
        }
    }

    #[test]
    fn test_parse_filter_complex() {
        let input = "FILTER WHERE age > 21 AND (city = 'NYC' OR city = 'LA')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Filter(c) => {
                assert_eq!(c.where_clause, "age > 21 AND (city = 'NYC' OR city = 'LA')");
            }
            _ => panic!("Expected Filter variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = FilterCommand::new("salary > 50000", vec![]);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FILTER WHERE salary > 50000");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Filter(c) => {
                assert_eq!(c.where_clause, "salary > 50000");
            }
            _ => panic!("Expected Filter variant"),
        }
    }
}
