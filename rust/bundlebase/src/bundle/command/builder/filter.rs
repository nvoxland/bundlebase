//! Filter command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::FilterOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use datafusion::scalar::ScalarValue;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to filter rows with a SELECT query.
#[derive(Debug, Clone)]
pub struct FilterCommand {
    /// The SELECT query
    pub query: String,
    /// Parameters for the query ($1, $2, etc.)
    pub params: Vec<ScalarValue>,
}

impl FilterCommand {
    /// Create a new FilterCommand.
    pub fn new(query: impl Into<String>, params: Vec<ScalarValue>) -> Self {
        Self {
            query: query.into(),
            params,
        }
    }
}

impl CommandParsing for FilterCommand {
    fn rule() -> Rule {
        Rule::filter_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut query = None;

        for inner_pair in pair.into_inner() {
            if let Rule::filter_query = inner_pair.as_rule() {
                query = Some(inner_pair.as_str().trim().to_string());
            }
        }

        let query = query.ok_or_else(|| -> BundlebaseError {
            "FILTER statement missing query".into()
        })?;

        if query.is_empty() {
            return Err("FILTER query cannot be empty".into());
        }

        Ok(FilterCommand::new(query, vec![]))
    }

    fn to_statement(&self) -> String {
        format!("FILTER WITH {}", self.query)
    }
}

#[async_trait]
impl BundleBuilderCommand for FilterCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(FilterOp::new(&self.query, self.params.clone()).into())
            .await?;
        Ok(format!("Filtered: {}", self.query))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_filter_simple() {
        let input = "FILTER WITH SELECT * FROM bundle WHERE country = 'USA'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Filter(c) => {
                assert_eq!(c.query, "SELECT * FROM bundle WHERE country = 'USA'");
            }
            _ => panic!("Expected Filter variant"),
        }
    }

    #[test]
    fn test_parse_filter_complex() {
        let input = "FILTER WITH SELECT * FROM bundle WHERE age > 21 AND (city = 'NYC' OR city = 'LA')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::Filter(c) => {
                assert_eq!(c.query, "SELECT * FROM bundle WHERE age > 21 AND (city = 'NYC' OR city = 'LA')");
            }
            _ => panic!("Expected Filter variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = FilterCommand::new("SELECT * FROM bundle WHERE salary > 50000", vec![]);
        let statement = cmd.to_statement();
        assert_eq!(statement, "FILTER WITH SELECT * FROM bundle WHERE salary > 50000");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::Filter(c) => {
                assert_eq!(c.query, "SELECT * FROM bundle WHERE salary > 50000");
            }
            _ => panic!("Expected Filter variant"),
        }
    }
}
