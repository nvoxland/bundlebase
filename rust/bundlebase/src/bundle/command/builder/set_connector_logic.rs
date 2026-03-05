//! SetConnectorLogic command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::extract_string_content;
use crate::bundle::operation::SetConnectorLogicOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use std::collections::HashMap;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to set platform-specific implementation logic for a defined source.
///
/// This command always persists the logic into the bundle by creating a
/// `SetConnectorLogicOp`. Python type cannot be bundled — use
/// `SET TEMPORARY CONNECTOR LOGIC` for runtime-only logic instead.
#[derive(Debug, Clone)]
pub struct SetConnectorLogicCommand {
    /// Full dotted source name
    pub name: String,
    /// Source type: "lib", "java", "docker", or "ipc"
    pub source_type: String,
    /// Logic string (e.g., path to shared library or binary)
    pub logic: String,
    /// Platform in Docker-style os/arch
    pub platform: String,
}

impl SetConnectorLogicCommand {
    pub fn new(
        name: impl Into<String>,
        source_type: impl Into<String>,
        logic: impl Into<String>,
        platform: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            source_type: source_type.into(),
            logic: logic.into(),
            platform: platform.into(),
        }
    }
}

impl CommandParsing for SetConnectorLogicCommand {
    fn rule() -> Rule {
        Rule::set_connector_logic_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut args = HashMap::new();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::source_args => {
                    for arg_pair in inner_pair.into_inner() {
                        if arg_pair.as_rule() == Rule::source_arg_pair {
                            let mut key = None;
                            let mut value = None;
                            for part in arg_pair.into_inner() {
                                match part.as_rule() {
                                    Rule::identifier => {
                                        key = Some(part.as_str().to_string());
                                    }
                                    Rule::quoted_string => {
                                        value = Some(extract_string_content(part.as_str())?);
                                    }
                                    _ => {}
                                }
                            }
                            if let (Some(k), Some(v)) = (key, value) {
                                args.insert(k, v);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "SET CONNECTOR LOGIC missing source name".into()
        })?;

        let source_type = args.remove("type").ok_or_else(|| -> BundlebaseError {
            "SET CONNECTOR LOGIC requires 'type' argument".into()
        })?;

        let logic = args.remove("logic").ok_or_else(|| -> BundlebaseError {
            "SET CONNECTOR LOGIC requires 'logic' argument".into()
        })?;

        let platform = args.remove("platform").unwrap_or_else(|| "*/*".to_string());

        Ok(SetConnectorLogicCommand::new(name, source_type, logic, platform))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        let parts = vec![
            format!("type = {}", escape_string(&self.source_type)),
            format!("logic = {}", escape_string(&self.logic)),
            format!("platform = {}", escape_string(&self.platform)),
        ];
        format!("SET CONNECTOR LOGIC {} WITH ({})", self.name, parts.join(", "))
    }
}

#[async_trait]
impl BundleBuilderCommand for SetConnectorLogicCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // python type cannot be bundled — use SET TEMPORARY CONNECTOR LOGIC instead
        if self.source_type == "python" {
            return Err(
                "python type cannot be bundled. Use SET TEMPORARY CONNECTOR LOGIC instead.".into(),
            );
        }

        // Always create a persisted operation
        let op = SetConnectorLogicOp::new(
            self.name.clone(),
            self.source_type.clone(),
            self.logic.clone(),
            self.platform.clone(),
        );
        builder.apply_operation(op.into()).await?;

        Ok(format!("Set connector logic for: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_set_connector_logic() {
        let input = "SET CONNECTOR LOGIC acme.weather WITH (type = 'lib', logic = '/usr/lib/weather.so', platform = '*/*')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::SetConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.source_type, "lib");
                assert_eq!(c.logic, "/usr/lib/weather.so");
                assert_eq!(c.platform, "*/*");
            }
            _ => panic!("Expected SetConnectorLogic variant"),
        }
    }

    #[test]
    fn test_parse_set_connector_logic_ipc() {
        let input = "SET CONNECTOR LOGIC acme.weather WITH (type = 'ipc', logic = './weather-linux', platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::SetConnectorLogic(c) => {
                assert_eq!(c.source_type, "ipc");
                assert_eq!(c.logic, "./weather-linux");
                assert_eq!(c.platform, "linux/amd64");
            }
            _ => panic!("Expected SetConnectorLogic variant"),
        }
    }

    #[test]
    fn test_parse_set_connector_logic_roundtrip() {
        let cmd = SetConnectorLogicCommand::new(
            "acme.weather",
            "lib",
            "/usr/lib/weather.so",
            "*/*",
        );
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::SetConnectorLogic(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.source_type, "lib");
                assert_eq!(c.logic, "/usr/lib/weather.so");
                assert_eq!(c.platform, "*/*");
            }
            _ => panic!("Expected SetConnectorLogic variant"),
        }
    }
}
