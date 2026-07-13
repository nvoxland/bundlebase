//! DescribeColumn command implementation.

use crate::parser::{extract_identifier, extract_string_content, quote_identifier};
use crate::BundleBuilderCommand;
use crate::{CommandParsing, Rule};
use bundlebase::bundle::operation::DescribeColumnOp;
use bundlebase::bundle::BundleFacade;
use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;

/// Command to attach a description to a column (metadata; travels in the bundle
/// and is surfaced to agents/tools). Syntax: `DESCRIBE COLUMN <name> AS '<text>'`.
#[derive(Debug, Clone)]
pub struct DescribeColumnCommand {
    /// The column name
    pub name: String,
    /// The description text
    pub description: String,
}

impl DescribeColumnCommand {
    /// Create a new DescribeColumnCommand.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
        }
    }
}

impl CommandParsing for DescribeColumnCommand {
    fn rule() -> Rule {
        Rule::describe_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut description = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::identifier => name = Some(extract_identifier(&inner)),
                Rule::quoted_string => {
                    description = Some(extract_string_content(inner.as_str())?)
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DESCRIBE COLUMN statement missing column name".into()
        })?;
        let description = description.ok_or_else(|| -> BundlebaseError {
            "DESCRIBE COLUMN statement missing description".into()
        })?;

        Ok(DescribeColumnCommand::new(name, description))
    }

    fn to_statement(&self) -> String {
        use crate::parser::escape_string;
        format!(
            "DESCRIBE COLUMN {} AS {}",
            quote_identifier(&self.name),
            escape_string(&self.description)
        )
    }
}

impl BundleBuilderCommand for DescribeColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let column_id = builder
            .column_id(&self.name)
            .ok_or_else(|| BundlebaseError::from(format!("Column '{}' not found", self.name)))?;

        builder
            .apply_operation(DescribeColumnOp::setup(column_id, &self.description).into())
            .await?;
        Ok(format!("Described column: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_describe_column() {
        let cmd = parse_command("DESCRIBE COLUMN salary AS 'annual gross pay in USD'").unwrap();
        match cmd {
            BundleCommand::DescribeColumn(c) => {
                assert_eq!(c.name, "salary");
                assert_eq!(c.description, "annual gross pay in USD");
            }
            _ => panic!("Expected DescribeColumn variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DescribeColumnCommand::new("user id", "the customer's id (it's unique)");
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DescribeColumn(c) => {
                assert_eq!(c.name, "user id");
                assert_eq!(c.description, "the customer's id (it's unique)");
            }
            _ => panic!("Expected DescribeColumn variant"),
        }
    }
}
