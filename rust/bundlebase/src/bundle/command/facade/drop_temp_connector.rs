//! DropTempConnector command implementation (session-only).
//!
//! Removes a connector for the current session only, without creating a persisted
//! operation. Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.

use crate::bundle::command::parser::extract_string_content;
use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::connector_definition::Platform;
use crate::bundle::facade::BundleFacade;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::sync::Arc;

/// Command to drop a session-only connector (not persisted).
///
/// Unlike `DropConnectorCommand` which persists to the bundle,
/// this command only removes the entrypoint for the current session. It works
/// on both read-only `Bundle` and `BundleBuilder` via `BundleFacade`.
#[derive(Debug, Clone)]
pub struct DropTempConnectorCommand {
    /// Full dotted connector name
    pub name: String,
    /// Optional platform filter
    pub platform: Option<Platform>,
}

impl DropTempConnectorCommand {
    pub fn new(name: impl Into<String>, platform: Option<Platform>) -> Self {
        Self {
            name: name.into(),
            platform,
        }
    }

    /// Returns the Arrow schema for drop temporary connector output.
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]))
    }

    /// Returns the expected output shape.
    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }
}

impl CommandParsing for DropTempConnectorCommand {
    fn rule() -> Rule {
        Rule::drop_temp_connector_stmt
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
            "DROP TEMP CONNECTOR missing connector name".into()
        })?;

        Ok(DropTempConnectorCommand::new(name, platform))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        match &self.platform {
            Some(p) => format!(
                "DROP TEMP CONNECTOR {} FOR PLATFORM {}",
                self.name,
                escape_string(&p.to_string())
            ),
            None => format!("DROP TEMP CONNECTOR {}", self.name),
        }
    }
}

#[async_trait]
impl BundleFacadeCommand for DropTempConnectorCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let count = facade
            .drop_temp_connector(&self.name, self.platform.as_ref())
            .await?;

        match &self.platform {
            Some(p) => Ok(format!(
                "Dropped {} temporary connector entries for {} on platform {}",
                count, self.name, p
            )),
            None => Ok(format!(
                "Dropped {} temporary connector entries for {}",
                count, self.name
            )),
        }
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_temp_connector() {
        let input = "DROP TEMP CONNECTOR acme.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, None);
            }
            _ => panic!("Expected DropTempConnector variant"),
        }
    }

    #[test]
    fn test_parse_drop_temp_connector_with_platform() {
        let input = "DROP TEMP CONNECTOR acme.weather FOR PLATFORM 'linux/amd64'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, Some("linux/amd64".parse::<Platform>().unwrap()));
            }
            _ => panic!("Expected DropTempConnector variant"),
        }
    }

    #[test]
    fn test_parse_drop_temp_connector_roundtrip() {
        let cmd = DropTempConnectorCommand::new("acme.weather", None);
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, None);
            }
            _ => panic!("Expected DropTempConnector variant"),
        }
    }

    #[test]
    fn test_parse_drop_temp_connector_roundtrip_with_platform() {
        let cmd =
            DropTempConnectorCommand::new("acme.weather", Some("linux/amd64".parse().unwrap()));
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, Some("linux/amd64".parse::<Platform>().unwrap()));
            }
            _ => panic!("Expected DropTempConnector variant"),
        }
    }
}
