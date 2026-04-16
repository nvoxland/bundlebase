//! ImportTempConnector command implementation (session-only).
//!
//! Loads a connector at runtime only, without persisting an operation.
//! Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.
//! This is the right choice for `python:mod:Class` calls that cannot be bundled.

use crate::parser::{escape_string, extract_string_content};
use crate::response::OutputShape;
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use bundlebase_common::NamespacedName;
use bundlebase_common::Platform;
use bundlebase_udf::runtime::UdfRuntime;
use bundlebase_udf::ConnectorEntry;
use std::collections::HashMap;
use std::sync::Arc;

/// Command to load a connector at runtime only (not persisted).
///
/// Unlike `ImportConnectorCommand` which persists to the bundle,
/// this command only sets the entrypoint for the current session. It works
/// on both read-only `Bundle` and `BundleBuilder` via `BundleFacade`.
#[derive(Debug, Clone)]
pub struct ImportTempConnectorCommand {
    /// Full dotted connector name
    pub name: String,
    /// Full from string (e.g., "python::mod:Class", "ipc::./source")
    pub from: String,
    /// Platform in Docker-style os/arch
    pub platform: Platform,
}

impl ImportTempConnectorCommand {
    pub fn new(name: impl Into<String>, from: impl Into<String>, platform: Platform) -> Self {
        Self {
            name: name.into(),
            from: from.into(),
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
        let mut from = None;
        let mut args = HashMap::new();

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::quoted_string => {
                    from = Some(extract_string_content(inner_pair.as_str())?);
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

        let from = from.ok_or_else(|| -> BundlebaseError {
            "IMPORT TEMP CONNECTOR missing FROM clause".into()
        })?;

        let platform: Platform = match args.remove("platform") {
            Some(s) => s.parse()?,
            None => Platform::any(),
        };

        Ok(ImportTempConnectorCommand::new(name, from, platform))
    }

    fn to_statement(&self) -> String {
        let mut with_parts = Vec::new();
        if self.platform != Platform::any() {
            with_parts.push(format!(
                "platform = {}",
                escape_string(&self.platform.to_string())
            ));
        }

        if with_parts.is_empty() {
            format!(
                "IMPORT TEMP CONNECTOR {} FROM {}",
                self.name,
                escape_string(&self.from)
            )
        } else {
            format!(
                "IMPORT TEMP CONNECTOR {} FROM {} WITH ({})",
                self.name,
                escape_string(&self.from),
                with_parts.join(", ")
            )
        }
    }
}

impl BundleFacadeCommand for ImportTempConnectorCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let from = UdfRuntime::parse_from(&self.from)?;
        from.validate_entrypoint()?;
        let namespaced: NamespacedName = self.name.parse()?;
        let entry = ConnectorEntry {
            id: bundlebase_data::ObjectId::generate(),
            name: namespaced,
            from,
            platform: self.platform,
            temporary: true,
        };
        facade.connector_registry().write().add_entry(entry);
        facade
            .function_registry()
            .read()
            .refresh_version_udf(facade.version());
        Ok(format!("Loaded temporary connector: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

    #[test]
    fn test_parse_import_temp_connector() {
        let input = "IMPORT TEMP CONNECTOR acme.weather FROM 'python::mod:Class'";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.from, "python::mod:Class");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_connector_roundtrip() {
        let cmd =
            ImportTempConnectorCommand::new("acme.weather", "python::mod:Class", Platform::any());
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "IMPORT TEMP CONNECTOR acme.weather FROM 'python::mod:Class'"
        );
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::ImportTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.from, "python::mod:Class");
                assert_eq!(c.platform, Platform::any());
            }
            _ => panic!("Expected ImportTempConnector variant"),
        }
    }

    #[test]
    fn test_parse_import_temp_connector_with_platform() {
        let input = "IMPORT TEMP CONNECTOR acme.weather FROM 'ipc::./source' WITH (platform = 'linux/amd64')";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::ImportTempConnector(c) => {
                assert_eq!(c.name, "acme.weather");
                assert_eq!(c.from, "ipc::./source");
                assert_eq!(c.platform.os, "linux");
                assert_eq!(c.platform.arch, "amd64");
            }
            _ => panic!("Expected ImportTempConnector variant"),
        }
    }
}
