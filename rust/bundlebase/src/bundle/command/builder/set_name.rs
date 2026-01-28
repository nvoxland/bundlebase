//! SetName command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::operation::SetNameOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to set the bundle's name.
#[derive(Debug, Clone)]
pub struct SetNameCommand {
    /// The name to set
    pub name: String,
}

impl SetNameCommand {
    /// Create a new SetNameCommand.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl CommandParsing for SetNameCommand {
    fn rule() -> Rule {
        Rule::set_name_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner in pair.into_inner() {
            if inner.as_rule() == Rule::quoted_string {
                name = Some(extract_string_content(inner.as_str())?);
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError { "SET NAME missing name".into() })?;

        Ok(SetNameCommand::new(name))
    }

    fn to_statement(&self) -> String {
        format!("SET NAME {}", escape_string(&self.name))
    }
}

#[async_trait]
impl BundleBuilderCommand for SetNameCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        builder
            .apply_operation(SetNameOp::setup(&self.name).into())
            .await?;
        Ok(format!("Set bundle name: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_set_name() {
        let input = "SET NAME 'My Bundle'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::SetName(c) => {
                assert_eq!(c.name, "My Bundle");
            }
            _ => panic!("Expected SetName variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = SetNameCommand::new("Test Bundle");
        let statement = cmd.to_statement();
        assert_eq!(statement, "SET NAME 'Test Bundle'");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SetName(c) => {
                assert_eq!(c.name, "Test Bundle");
            }
            _ => panic!("Expected SetName variant"),
        }
    }
}
