//! DropJoin command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::DropJoinOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to drop a join.
#[derive(Debug, Clone)]
pub struct DropJoinCommand {
    /// The name of the join to drop
    pub name: String,
}

impl DropJoinCommand {
    /// Create a new DropJoinCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl CommandParsing for DropJoinCommand {
    fn rule() -> Rule {
        Rule::drop_join_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::identifier {
                name = Some(inner_pair.as_str().to_string());
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DROP JOIN statement missing join name".into()
        })?;

        Ok(DropJoinCommand::new(name))
    }

    fn to_statement(&self) -> String {
        format!("DROP JOIN {}", self.name)
    }
}

#[async_trait]
impl BundleBuilderCommand for DropJoinCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DropJoinOp::setup(&self.name, builder).await?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Dropped join: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_join() {
        let input = "DROP JOIN customers";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropJoin(c) => {
                assert_eq!(c.name, "customers");
            }
            _ => panic!("Expected DropJoin variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DropJoinCommand::new("my_join");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP JOIN my_join");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropJoin(c) => {
                assert_eq!(c.name, "my_join");
            }
            _ => panic!("Expected DropJoin variant"),
        }
    }
}
