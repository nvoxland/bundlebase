//! DropConnector command implementation.

use crate::bundle::command::parser::extract_string_content;
use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::connector_definition::Platform;
use crate::bundle::operation::DropConnectorOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to drop a connector definition and all its logic, or drop logic for a specific platform.
///
/// Without a platform, removes the entire connector definition, all logic, and sources.
/// With a platform, removes only the logic entry for that platform.
#[derive(Debug, Clone)]
pub struct DropConnectorCommand {
    /// Full dotted source name
    pub name: String,
    /// Optional platform filter
    pub platform: Option<Platform>,
}

impl DropConnectorCommand {
    pub fn new(name: impl Into<String>, platform: Option<Platform>) -> Self {
        Self {
            name: name.into(),
            platform,
        }
    }
}

impl CommandParsing for DropConnectorCommand {
    fn rule() -> Rule {
        Rule::drop_connector_stmt
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
                    let s = extract_string_content(inner_pair.as_str())?;
                    platform = Some(s.parse::<Platform>()?);
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DROP CONNECTOR missing connector name".into()
        })?;

        Ok(DropConnectorCommand::new(name, platform))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        match &self.platform {
            Some(p) => format!(
                "DROP CONNECTOR {} FOR PLATFORM {}",
                self.name,
                escape_string(&p.to_string())
            ),
            None => format!("DROP CONNECTOR {}", self.name),
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for DropConnectorCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = DropConnectorOp::new(self.name.clone(), self.platform.clone());
        builder.apply_operation(op.into()).await?;

        match &self.platform {
            Some(p) => Ok(format!(
                "Dropped connector logic for {} on platform {}",
                self.name, p
            )),
            None => Ok(format!("Dropped connector {}", self.name)),
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::connector_definition::Platform;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_connector() {
        let input = "DROP CONNECTOR acme.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, None);
            }
            _ => panic!("Expected DropConnector variant"),
        }
    }

    #[test]
    fn test_parse_drop_connector_with_platform() {
        let input = "DROP CONNECTOR acme.weather FOR PLATFORM 'linux/amd64'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, Some("linux/amd64".parse::<Platform>().unwrap()));
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
                assert_eq!(c.platform, None);
            }
            _ => panic!("Expected DropConnector variant"),
        }
    }

    #[test]
    fn test_parse_drop_connector_roundtrip() {
        let cmd = DropConnectorCommand::new("acme.weather", None);
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, None);
            }
            _ => panic!("Expected DropConnector variant"),
        }
    }

    #[test]
    fn test_parse_drop_connector_roundtrip_with_platform() {
        let cmd = DropConnectorCommand::new("acme.weather", Some("linux/amd64".parse().unwrap()));
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, Some("linux/amd64".parse::<Platform>().unwrap()));
            }
            _ => panic!("Expected DropConnector variant"),
        }
    }
}
