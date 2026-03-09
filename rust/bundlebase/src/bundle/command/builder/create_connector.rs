//! CreateConnector command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::extract_string_content;
use crate::bundle::connector_definition::{Platform, Runner};
use crate::bundle::operation::CreateConnectorOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use std::collections::HashMap;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to define a named connector with its logic.
///
/// Combines connector creation and logic setting into a single command.
/// If the connector already exists, adds/replaces logic for the given platform.
#[derive(Debug, Clone)]
pub struct CreateConnectorCommand {
    /// Full dotted source name (e.g., "acme.weather")
    pub name: String,
    /// Runner type
    pub runner: Runner,
    /// Logic string (e.g., path to shared library or binary)
    pub logic: String,
    /// Platform in Docker-style os/arch
    pub platform: Platform,
}

impl CreateConnectorCommand {
    pub fn new(
        name: impl Into<String>,
        runner: Runner,
        logic: impl Into<String>,
        platform: Platform,
    ) -> Self {
        Self {
            name: name.into(),
            runner,
            logic: logic.into(),
            platform,
        }
    }
}

impl CommandParsing for CreateConnectorCommand {
    fn rule() -> Rule {
        Rule::create_connector_stmt
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
            "CREATE CONNECTOR missing connector name".into()
        })?;

        let runner_str = args.remove("runner").ok_or_else(|| -> BundlebaseError {
            "CREATE CONNECTOR requires 'runner' argument".into()
        })?;
        let runner: Runner = runner_str.parse()?;

        let logic = args.remove("logic").ok_or_else(|| -> BundlebaseError {
            "CREATE CONNECTOR requires 'logic' argument".into()
        })?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        Ok(CreateConnectorCommand::new(name, runner, logic, platform))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        let runner_str = self.runner.to_string();
        let parts = vec![
            format!("runner = {}", escape_string(&runner_str)),
            format!("logic = {}", escape_string(&self.logic)),
            format!("platform = {}", escape_string(&self.platform.to_string())),
        ];
        format!("CREATE CONNECTOR {} WITH ({})", self.name, parts.join(", "))
    }
}

#[async_trait]
impl BundleBuilderCommand for CreateConnectorCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = CreateConnectorOp::new(
            self.name.clone(),
            self.runner,
            self.logic.clone(),
            self.platform.clone(),
        );
        builder.apply_operation(op.into()).await?;

        Ok(format!("Created connector: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::connector_definition::Platform;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_connector() {
        let input = "CREATE CONNECTOR acme.weather WITH (runner = 'ipc', logic = './my_source')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_source");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected CreateConnector variant"),
        }
    }

    #[test]
    fn test_parse_create_connector_with_platform() {
        let input = "CREATE CONNECTOR acme.weather WITH (runner = 'lib', logic = './lib.so', platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.logic, "./lib.so");
                assert_eq!(c.platform, "linux/amd64".parse::<Platform>().unwrap());
            }
            _ => panic!("Expected CreateConnector variant"),
        }
    }

    #[test]
    fn test_parse_create_connector_deep_name_parses_but_check_rejects() {
        // Multi-level names parse fine at grammar level but are rejected by operation check()
        let input = "CREATE CONNECTOR acme.datasources.weather WITH (runner = 'ipc', logic = './weather')";
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
        let cmd = CreateConnectorCommand::new(
            "acme.weather",
            Runner::Lib,
            "/usr/lib/weather.so",
            Platform::any(),
        );
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.logic, "/usr/lib/weather.so");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected CreateConnector variant"),
        }
    }

    #[test]
    fn test_parse_create_connector_case_insensitive() {
        let input = "create connector acme.weather with (runner = 'ipc', logic = './test')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Ipc);
            }
            _ => panic!("Expected CreateConnector variant"),
        }
    }
}
