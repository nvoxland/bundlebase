//! CreateTemporaryConnector command implementation (runtime-only).
//!
//! Creates a connector with runtime-only logic, without persisting an operation.
//! Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.
//! This is the right choice for `python:mod:Class` calls that cannot be bundled.

use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::command::parser::extract_string_content;
use crate::bundle::facade::BundleFacade;
use crate::bundle::connector_definition::ConnectorLogicEntry;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Command to create a connector with runtime-only logic (not persisted).
///
/// Unlike `CreateConnectorCommand` which persists to the bundle,
/// this command only sets logic for the current session. It works
/// on both read-only `Bundle` and `BundleBuilder` via `BundleFacade`.
#[derive(Debug, Clone)]
pub struct CreateTemporaryConnectorCommand {
    /// Full dotted source name
    pub name: String,
    /// Runner: "python", "lib", "java", "docker", or "ipc"
    pub runner: String,
    /// Logic string (e.g., "mod:Class" for python, path for lib/ipc)
    pub logic: String,
    /// Platform in Docker-style os/arch
    pub platform: String,
}

impl CreateTemporaryConnectorCommand {
    pub fn new(
        name: impl Into<String>,
        runner: impl Into<String>,
        logic: impl Into<String>,
        platform: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            runner: runner.into(),
            logic: logic.into(),
            platform: platform.into(),
        }
    }

    /// Returns the Arrow schema for create temporary connector output.
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

impl CommandParsing for CreateTemporaryConnectorCommand {
    fn rule() -> Rule {
        Rule::create_temporary_connector_stmt
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
            "CREATE TEMPORARY CONNECTOR missing connector name".into()
        })?;

        let runner = args.remove("runner").ok_or_else(|| -> BundlebaseError {
            "CREATE TEMPORARY CONNECTOR requires 'runner' argument".into()
        })?;

        let logic = args.remove("logic").ok_or_else(|| -> BundlebaseError {
            "CREATE TEMPORARY CONNECTOR requires 'logic' argument".into()
        })?;

        let platform = args.remove("platform").unwrap_or_else(|| "*/*".to_string());

        Ok(CreateTemporaryConnectorCommand::new(name, runner, logic, platform))
    }

    fn to_statement(&self) -> String {
        use crate::bundle::command::parser::escape_string;
        let parts = vec![
            format!("runner = {}", escape_string(&self.runner)),
            format!("logic = {}", escape_string(&self.logic)),
            format!("platform = {}", escape_string(&self.platform)),
        ];
        format!(
            "CREATE TEMPORARY CONNECTOR {} WITH ({})",
            self.name,
            parts.join(", ")
        )
    }
}

#[async_trait]
impl BundleFacadeCommand for CreateTemporaryConnectorCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let entry = ConnectorLogicEntry {
            runner: self.runner.clone(),
            logic: self.logic.clone(),
            platform: self.platform.clone(),
        };
        facade.create_temporary_connector(&self.name, entry).await?;
        Ok(format!("Created temporary connector: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_create_temporary_connector() {
        let input = "CREATE TEMPORARY CONNECTOR acme.weather WITH (runner = 'python', logic = 'mod:Class')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::CreateTemporaryConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, "python");
                assert_eq!(c.logic, "mod:Class");
                assert_eq!(c.platform, "*/*");
            }
            _ => panic!("Expected CreateTemporaryConnector variant"),
        }
    }

    #[test]
    fn test_parse_create_temporary_connector_roundtrip() {
        let cmd = CreateTemporaryConnectorCommand::new(
            "acme.weather",
            "python",
            "mod:Class",
            "*/*",
        );
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::CreateTemporaryConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.runner, "python");
                assert_eq!(c.logic, "mod:Class");
                assert_eq!(c.platform, "*/*");
            }
            _ => panic!("Expected CreateTemporaryConnector variant"),
        }
    }
}
