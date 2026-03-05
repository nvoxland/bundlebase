//! DropConnector command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::DropConnectorOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to drop a defined connector and all its associated logic and sources.
#[derive(Debug, Clone)]
pub struct DropConnectorCommand {
    /// Full dotted source name
    pub name: String,
}

impl DropConnectorCommand {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl CommandParsing for DropConnectorCommand {
    fn rule() -> Rule {
        Rule::drop_connector_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    name = Some(inner_pair.as_str().to_string());
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DROP CONNECTOR missing source name".into()
        })?;

        Ok(DropConnectorCommand::new(name))
    }

    fn to_statement(&self) -> String {
        format!("DROP CONNECTOR {}", self.name)
    }
}

#[async_trait]
impl BundleBuilderCommand for DropConnectorCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DropConnectorOp::new(self.name.clone());
        builder.apply_operation(op.into()).await?;
        Ok(format!("Dropped connector {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_connector() {
        let input = "DROP CONNECTOR acme.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected DropConnector variant"),
        }
    }

    #[test]
    fn test_parse_drop_connector_case_insensitive() {
        let input = "drop connector acme.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected DropConnector variant"),
        }
    }

    #[test]
    fn test_parse_drop_connector_roundtrip() {
        let cmd = DropConnectorCommand::new("acme.weather");
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected DropConnector variant"),
        }
    }
}
