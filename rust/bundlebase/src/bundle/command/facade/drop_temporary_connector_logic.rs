//! DropTemporaryConnectorLogic command implementation (runtime-only).
//!
//! Removes connector logic for the current session only, without creating a persisted
//! operation. Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.

use crate::bundle::command::parser::extract_string_content;
use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::facade::BundleFacade;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::sync::Arc;

/// Command to drop runtime-only connector logic (not persisted).
///
/// Unlike `DropConnectorCommand` which persists to the bundle,
/// this command only removes logic for the current session. It works
/// on both read-only `Bundle` and `BundleBuilder` via `BundleFacade`.
#[derive(Debug, Clone)]
pub struct DropTemporaryConnectorLogicCommand {
    /// Full dotted source name
    pub name: String,
    /// Optional platform filter
    pub platform: Option<String>,
}

impl DropTemporaryConnectorLogicCommand {
    pub fn new(name: impl Into<String>, platform: Option<String>) -> Self {
        Self {
            name: name.into(),
            platform,
        }
    }

    /// Returns the Arrow schema for drop temporary connector logic output.
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

impl CommandParsing for DropTemporaryConnectorLogicCommand {
    fn rule() -> Rule {
        Rule::drop_temporary_connector_logic_stmt
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
            "DROP TEMPORARY CONNECTOR LOGIC missing source name".into()
        })?;

        Ok(DropTemporaryConnectorLogicCommand::new(name, platform))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        match &self.platform {
            Some(p) => format!(
                "DROP TEMPORARY CONNECTOR LOGIC {} FOR PLATFORM {}",
                self.name,
                escape_string(p)
            ),
            None => format!("DROP TEMPORARY CONNECTOR LOGIC {}", self.name),
        }
    }
}

#[async_trait]
impl BundleFacadeCommand for DropTemporaryConnectorLogicCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let count = facade
            .drop_temporary_connector_logic(&self.name, self.platform.as_deref())
            .await?;

        match &self.platform {
            Some(p) => Ok(format!(
                "Dropped {} temporary connector logic entries for {} on platform {}",
                count, self.name, p
            )),
            None => Ok(format!(
                "Dropped {} temporary connector logic entries for {}",
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
    fn test_parse_drop_temporary_connector_logic() {
        let input = "DROP TEMPORARY CONNECTOR LOGIC acme.weather";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropTemporaryConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, None);
            }
            _ => panic!("Expected DropTemporaryConnectorLogic variant"),
        }
    }

    #[test]
    fn test_parse_drop_temporary_connector_logic_with_platform() {
        let input = "DROP TEMPORARY CONNECTOR LOGIC acme.weather FOR PLATFORM 'linux/amd64'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropTemporaryConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, Some("linux/amd64".to_string()));
            }
            _ => panic!("Expected DropTemporaryConnectorLogic variant"),
        }
    }

    #[test]
    fn test_parse_drop_temporary_connector_logic_roundtrip() {
        let cmd = DropTemporaryConnectorLogicCommand::new("acme.weather", None);
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropTemporaryConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, None);
            }
            _ => panic!("Expected DropTemporaryConnectorLogic variant"),
        }
    }

    #[test]
    fn test_parse_drop_temporary_connector_logic_roundtrip_with_platform() {
        let cmd =
            DropTemporaryConnectorLogicCommand::new("acme.weather", Some("linux/amd64".to_string()));
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropTemporaryConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.platform, Some("linux/amd64".to_string()));
            }
            _ => panic!("Expected DropTemporaryConnectorLogic variant"),
        }
    }
}
