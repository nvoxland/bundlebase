//! DropConnectorLogic command implementation.

use crate::bundle::command::parser::extract_string_content;
use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::operation::DropConnectorLogicOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to drop persisted connector logic for a defined source.
///
/// Creates a `DropConnectorLogicOp` that removes logic entries from the source definition.
/// If `platform` is None, all logic entries are dropped.
#[derive(Debug, Clone)]
pub struct DropConnectorLogicCommand {
    /// Full dotted source name
    pub name: String,
    /// Optional platform filter
    pub platform: Option<String>,
}

impl DropConnectorLogicCommand {
    pub fn new(name: impl Into<String>, platform: Option<String>) -> Self {
        Self {
            name: name.into(),
            platform,
        }
    }
}

impl CommandParsing for DropConnectorLogicCommand {
    fn rule() -> Rule {
        Rule::drop_connector_logic_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut platform = None;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::quoted_string => {
                    platform = Some(extract_string_content(inner_pair.as_str())?);
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DROP CONNECTOR LOGIC missing source name".into()
        })?;

        Ok(DropConnectorLogicCommand::new(name, platform))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        match &self.platform {
            Some(p) => format!(
                "DROP CONNECTOR LOGIC {} FOR PLATFORM {}",
                self.name,
                escape_string(p)
            ),
            None => format!("DROP CONNECTOR LOGIC {}", self.name),
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for DropConnectorLogicCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DropConnectorLogicOp::new(self.name.clone(), self.platform.clone());
        builder.apply_operation(op.into()).await?;

        match &self.platform {
            Some(p) => Ok(format!(
                "Dropped connector logic for {} on platform {}",
                self.name, p
            )),
            None => Ok(format!("Dropped all connector logic for {}", self.name)),
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_connector_logic() {
        let input = "DROP CONNECTOR LOGIC acme.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, None);
            }
            _ => panic!("Expected DropConnectorLogic variant"),
        }
    }

    #[test]
    fn test_parse_drop_connector_logic_with_platform() {
        let input = "DROP CONNECTOR LOGIC acme.weather FOR PLATFORM 'linux/amd64'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, Some("linux/amd64".to_string()));
            }
            _ => panic!("Expected DropConnectorLogic variant"),
        }
    }

    #[test]
    fn test_parse_drop_connector_logic_roundtrip() {
        let cmd = DropConnectorLogicCommand::new("acme.weather", None);
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, None);
            }
            _ => panic!("Expected DropConnectorLogic variant"),
        }
    }

    #[test]
    fn test_parse_drop_connector_logic_roundtrip_with_platform() {
        let cmd = DropConnectorLogicCommand::new("acme.weather", Some("linux/amd64".to_string()));
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, Some("linux/amd64".to_string()));
            }
            _ => panic!("Expected DropConnectorLogic variant"),
        }
    }
}
