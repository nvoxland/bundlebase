//! DropView command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::DropViewOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to drop a view.
#[derive(Debug, Clone)]
pub struct DropViewCommand {
    /// The name of the view to drop
    pub name: String,
}

impl DropViewCommand {
    /// Create a new DropViewCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl CommandParsing for DropViewCommand {
    fn rule() -> Rule {
        Rule::drop_view_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::identifier {
                name = Some(inner.as_str().to_string());
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError { "DROP VIEW missing name".into() })?;

        Ok(DropViewCommand::new(name))
    }

    fn to_statement(&self) -> String {
        format!("DROP VIEW {}", self.name)
    }
}

#[async_trait]
impl BundleBuilderCommand for DropViewCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DropViewOp::setup(&self.name, builder).await?;
        builder.apply_operation(op.into()).await?;
        Ok(format!("Dropped view: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_view() {
        let input = "DROP VIEW summary";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropView(c) => {
                assert_eq!(c.name, "summary");
            }
            _ => panic!("Expected DropView variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = DropViewCommand::new("my_view");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP VIEW my_view");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropView(c) => {
                assert_eq!(c.name, "my_view");
            }
            _ => panic!("Expected DropView variant"),
        }
    }
}
