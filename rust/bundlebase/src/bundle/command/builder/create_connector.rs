//! CreateConnector command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::CreateConnectorOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to define a named connector.
#[derive(Debug, Clone)]
pub struct CreateConnectorCommand {
    /// Full dotted source name (e.g., "acme.datasources.weather")
    pub name: String,
}

impl CreateConnectorCommand {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl CommandParsing for CreateConnectorCommand {
    fn rule() -> Rule {
        Rule::create_connector_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::dotted_identifier {
                name = Some(inner_pair.as_str().to_string());
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "CREATE CONNECTOR missing source name".into()
        })?;

        Ok(CreateConnectorCommand::new(name))
    }

    fn to_statement(&self) -> String {
        format!("CREATE CONNECTOR {}", self.name)
    }
}

#[async_trait]
impl BundleBuilderCommand for CreateConnectorCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = CreateConnectorOp::new(self.name.clone());
        builder.apply_operation(op.into()).await?;
        Ok(format!("Created connector: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_connector() {
        let input = "CREATE CONNECTOR acme.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected CreateConnector variant"),
        }
    }

    #[test]
    fn test_parse_create_connector_deep() {
        let input = "CREATE CONNECTOR acme.datasources.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateConnector(c) => {
                assert_eq!(c.name, "acme.datasources.weather");
            }
            _ => panic!("Expected CreateConnector variant"),
        }
    }

    #[test]
    fn test_parse_create_connector_roundtrip() {
        let cmd = CreateConnectorCommand::new("acme.datasources.weather");
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateConnector(c) => {
                assert_eq!(c.name, "acme.datasources.weather");
            }
            _ => panic!("Expected CreateConnector variant"),
        }
    }

    #[test]
    fn test_parse_create_connector_case_insensitive() {
        let input = "create connector acme.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected CreateConnector variant"),
        }
    }
}
