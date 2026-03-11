//! ImportTempConnector command implementation (runtime-only).
//!
//! Loads a connector with runtime-only logic, without persisting an operation.
//! Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.
//! This is the right choice for `python:mod:Class` calls that cannot be bundled.

use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::command::parser::{escape_string, extract_string_content};
use crate::bundle::facade::BundleFacade;
use crate::bundle::connector_definition::{parse_from_url, to_from_url, Platform, Runner};
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Command to load a connector with runtime-only logic (not persisted).
///
/// Unlike `ImportConnectorCommand` which persists to the bundle,
/// this command only sets logic for the current session. It works
/// on both read-only `Bundle` and `BundleBuilder` via `BundleFacade`.
#[derive(Debug, Clone)]
pub struct ImportTempConnectorCommand {
    /// Full dotted connector name
    pub name: String,
    /// Runner type
    pub runner: Runner,
    /// Logic string (e.g., "mod:Class" for python, path for lib/ipc)
    pub logic: String,
    /// Platform in Docker-style os/arch
    pub platform: Platform,
}

impl ImportTempConnectorCommand {
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

    /// Returns the Arrow schema for load temporary connector output.
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

impl CommandParsing for ImportTempConnectorCommand {
    fn rule() -> Rule {
        Rule::import_temp_connector_stmt
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
            "IMPORT TEMP CONNECTOR missing connector name".into()
        })?;

        let from_url = from_url.ok_or_else(|| -> BundlebaseError {
            "IMPORT TEMP CONNECTOR missing FROM clause".into()
        })?;

        let (runner, logic) = parse_from_url(&from_url)?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        Ok(ImportTempConnectorCommand::new(name, runner, logic, platform))
    }

    fn to_statement(&self) -> String {
        let from_url = to_from_url(self.runner, &self.logic);
        let mut with_parts = Vec::new();
        if self.platform != Platform::any() {
            with_parts.push(format!("platform = {}", escape_string(&self.platform.to_string())));
        }

        if with_parts.is_empty() {
            format!(
                "IMPORT TEMP CONNECTOR {} FROM {}",
                self.name,
                escape_string(&from_url)
            )
        } else {
            format!(
                "IMPORT TEMP CONNECTOR {} FROM {} WITH ({})",
                self.name,
                escape_string(&from_url),
                with_parts.join(", ")
            )
        }
    }
}

#[async_trait]
impl BundleFacadeCommand for ImportTempConnectorCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        facade.import_temp_connector(&self.name, self.runner, self.logic.clone(), self.platform).await?;
        Ok(format!("Loaded temporary connector: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_import_temp_connector() {
        let input = "IMPORT TEMP CONNECTOR acme.weather FROM 'python://mod:Class'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Python);
                assert_eq!(c.logic, "mod:Class");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_connector_roundtrip() {
        let cmd = ImportTempConnectorCommand::new(
            "acme.weather",
            Runner::Python,
            "mod:Class",
            Platform::any(),
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "IMPORT TEMP CONNECTOR acme.weather FROM 'python://mod:Class'");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Python);
                assert_eq!(c.logic, "mod:Class");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_connector_with_platform() {
        let input = "IMPORT TEMP CONNECTOR acme.weather FROM 'ipc://./source' WITH (platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, Runner::Ipc);
                assert_eq!(c.logic, "./source");
                assert_eq!(c.platform.os, "linux");
                assert_eq!(c.platform.arch, "amd64");
            }
            _ => panic!("Expected ImportTempConnector variant"),
        }
    }
}
