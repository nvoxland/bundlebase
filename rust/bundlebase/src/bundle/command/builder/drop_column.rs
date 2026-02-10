//! DropColumn command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::DropColumnOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to drop a column from the bundle.
#[derive(Debug, Clone)]
pub struct DropColumnCommand {
    /// The column name to drop
    pub name: String,
}

impl DropColumnCommand {
    /// Create a new DropColumnCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl CommandParsing for DropColumnCommand {
    fn rule() -> Rule {
        Rule::drop_column_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                name = Some(inner.as_str().to_string());
            }
        }

        let name =
            name.ok_or_else(|| -> BundlebaseError { "DROP COLUMN missing column name".into() })?;

        Ok(DropColumnCommand::new(name))
    }

    fn to_statement(&self) -> String {
        format!("DROP COLUMN {}", self.name)
    }
}

#[async_trait]
impl BundleBuilderCommand for DropColumnCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(DropColumnOp::setup(vec![self.name.as_str()]).into())
            .await?;

        Ok(format!("Dropped column: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_column() {
        let input = "DROP COLUMN old_column";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropColumn(c) => {
                assert_eq!(c.name, "old_column");
            }
            _ => panic!("Expected DropColumn variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DropColumnCommand::new("temp_col");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP COLUMN temp_col");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropColumn(c) => {
                assert_eq!(c.name, "temp_col");
            }
            _ => panic!("Expected DropColumn variant"),
        }
    }
}
