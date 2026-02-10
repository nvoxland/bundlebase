//! DropIndex command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::DropIndexOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to drop an index on a column.
#[derive(Debug, Clone)]
pub struct DropIndexCommand {
    /// The column whose index should be dropped
    pub column: String,
}

impl DropIndexCommand {
    /// Create a new DropIndexCommand.
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
        }
    }
}

impl CommandParsing for DropIndexCommand {
    fn rule() -> Rule {
        Rule::drop_index_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut column = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::identifier {
                column = Some(inner_pair.as_str().to_string());
            }
        }

        let column = column.ok_or_else(|| -> BundlebaseError {
            "DROP INDEX statement missing column name".into()
        })?;

        Ok(DropIndexCommand::new(column))
    }

    fn to_statement(&self) -> String {
        format!("DROP INDEX {}", self.column)
    }
}

#[async_trait]
impl BundleBuilderCommand for DropIndexCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Find the index ID for the given column
        let index_id = {
            let bundle = builder.bundle();
            let indexes = bundle.indexes().read();
            let index = indexes
                .iter()
                .find(|idx| idx.column() == self.column.as_str());

            match index {
                Some(idx) => *idx.id(),
                None => {
                    return Err(format!("No index found for column '{}'", self.column).into());
                }
            }
        };

        builder
            .apply_operation(DropIndexOp::setup(&index_id).await?.into())
            .await?;

        Ok(format!("Dropped index on column: {}", self.column))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_index() {
        let input = "DROP INDEX user_id";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropIndex(c) => {
                assert_eq!(c.column, "user_id");
            }
            _ => panic!("Expected DropIndex variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DropIndexCommand::new("email");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP INDEX email");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropIndex(c) => {
                assert_eq!(c.column, "email");
            }
            _ => panic!("Expected DropIndex variant"),
        }
    }
}
