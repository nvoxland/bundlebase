//! RenameConnector command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::RenameConnectorOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to rename a connector.
///
/// Renames all entries for a connector name to a new dotted name.
/// Sources referencing the old connector name are updated to point
/// to the new name.
#[derive(Debug, Clone)]
pub struct RenameConnectorCommand {
    /// The current connector name (dotted, e.g. "acme.weather")
    pub old_name: String,
    /// The new connector name (dotted, e.g. "acme.weather_v2")
    pub new_name: String,
}

impl RenameConnectorCommand {
    /// Create a new RenameConnectorCommand.
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }
}

impl CommandParsing for RenameConnectorCommand {
    fn rule() -> Rule {
        Rule::rename_connector_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut old_name = None;
        let mut new_name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::dotted_identifier {
                if old_name.is_none() {
                    old_name = Some(inner_pair.as_str().to_string());
                } else {
                    new_name = Some(inner_pair.as_str().to_string());
                }
            }
        }

        let old_name = old_name.ok_or_else(|| -> BundlebaseError {
            "RENAME CONNECTOR statement missing old name".into()
        })?;
        let new_name = new_name.ok_or_else(|| -> BundlebaseError {
            "RENAME CONNECTOR statement missing new name".into()
        })?;

        Ok(RenameConnectorCommand::new(old_name, new_name))
    }

    fn to_statement(&self) -> String {
        format!("RENAME CONNECTOR {} TO {}", self.old_name, self.new_name)
    }
}

#[async_trait]
impl BundleBuilderCommand for RenameConnectorCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = RenameConnectorOp::setup(&self.old_name, &self.new_name, builder)?;
        builder.apply_operation(op.into()).await?;
        Ok(format!(
            "Renamed connector: {} to {}",
            self.old_name, self.new_name
        ))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_rename_connector() {
        let input = "RENAME CONNECTOR acme.weather TO acme.weather_v2";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameConnector(c) => {
                assert_eq!(c.old_name, "acme.weather");
                assert_eq!(c.new_name, "acme.weather_v2");
            }
            _ => panic!("Expected RenameConnector variant"),
        }
    }

    #[test]
    fn test_parse_rename_connector_case_insensitive() {
        let input = "rename connector acme.weather to acme.weather_v2";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameConnector(c) => {
                assert_eq!(c.old_name, "acme.weather");
                assert_eq!(c.new_name, "acme.weather_v2");
            }
            _ => panic!("Expected RenameConnector variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = RenameConnectorCommand::new("acme.weather", "acme.weather_v2");
        let statement = cmd.to_statement();
        assert_eq!(statement, "RENAME CONNECTOR acme.weather TO acme.weather_v2");

        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RenameConnector(c) => {
                assert_eq!(c.old_name, "acme.weather");
                assert_eq!(c.new_name, "acme.weather_v2");
            }
            _ => panic!("Expected RenameConnector variant"),
        }
    }
}
