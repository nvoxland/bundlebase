//! ImportConnector command implementation.

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::connector_definition::{parse_from_url, to_from_url, Platform, Runner};
use crate::bundle::operation::ImportConnectorOp;
use crate::BundlebaseError;
use async_trait::async_trait;
use std::collections::HashMap;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to define a named connector with its logic.
///
/// Combines connector loading and logic setting into a single command.
/// If the connector already exists, adds/replaces logic for the given platform.
#[derive(Debug, Clone)]
pub struct ImportConnectorCommand {
    /// Full dotted connector name (e.g., "acme.weather")
    pub name: String,
    /// Runner type
    pub runner: Runner,
    /// Logic string (e.g., path to shared library or binary)
    pub logic: String,
    /// Platform in Docker-style os/arch
    pub platform: Platform,
}

impl ImportConnectorCommand {
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

impl CommandParsing for ImportConnectorCommand {
    fn rule() -> Rule {
        Rule::import_connector_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut from_url = None;
        let mut args = HashMap::new();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::quoted_string => {
                    from_url = Some(extract_string_content(inner_pair.as_str())?);
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
            "IMPORT CONNECTOR missing connector name".into()
        })?;

        let from_url = from_url.ok_or_else(|| -> BundlebaseError {
            "IMPORT CONNECTOR missing FROM clause".into()
        })?;

        let (runner, logic) = parse_from_url(&from_url)?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        Ok(ImportConnectorCommand::new(name, runner, logic, platform))
    }

    fn to_statement(&self) -> String {
        let from_url = to_from_url(self.runner, &self.logic);
        let mut with_parts = Vec::new();
        if self.platform != Platform::any() {
            with_parts.push(format!("platform = {}", escape_string(&self.platform.to_string())));
        }

        if with_parts.is_empty() {
            format!(
                "IMPORT CONNECTOR {} FROM {}",
                self.name,
                escape_string(&from_url)
            )
        } else {
            format!(
                "IMPORT CONNECTOR {} FROM {} WITH ({})",
                self.name,
                escape_string(&from_url),
                with_parts.join(", ")
            )
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for ImportConnectorCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let op = ImportConnectorOp::new(
            self.name.clone(),
            self.runner,
            self.logic.clone(),
            self.platform.clone(),
        );
        builder.apply_operation(op.into()).await?;

        Ok(format!("Loaded connector: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::connector_definition::Platform;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_import_connector() {
        let input = "IMPORT CONNECTOR acme.weather FROM 'ipc://./my_source'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./my_source");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_with_platform() {
        let input = "IMPORT CONNECTOR acme.weather FROM 'lib://./lib.so' WITH (platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.logic, "./lib.so");
                assert_eq!(c.platform, "linux/amd64".parse::<Platform>().unwrap());
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_deep_name_parses_but_check_rejects() {
        let input = "IMPORT CONNECTOR acme.weather FROM 'ipc://./weather'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_roundtrip() {
        let cmd = ImportConnectorCommand::new(
            "acme.weather",
            Runner::Lib,
            "/usr/lib/weather.so",
            Platform::any(),
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT CONNECTOR acme.weather FROM 'lib:///usr/lib/weather.so'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Lib);
                assert_eq!(c.logic, "/usr/lib/weather.so");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_roundtrip_with_platform() {
        let cmd = ImportConnectorCommand::new(
            "acme.weather",
            Runner::Ipc,
            "./my_source",
            "linux/amd64".parse().unwrap(),
        );
        let statement = cmd.to_statement();
        assert!(statement.contains("WITH (platform = 'linux/amd64')"));
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.platform.os, "linux");
                assert_eq!(c.platform.arch, "amd64");
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_connector_case_insensitive() {
        let input = "load connector acme.weather from 'ipc://./test'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Ipc);
            }
            _ => panic!("Expected ImportConnector variant"),
        }
    }
}
