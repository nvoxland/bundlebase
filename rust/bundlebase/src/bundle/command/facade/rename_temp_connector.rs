//! RenameTempConnector command implementation (runtime-only).
//!
//! Renames temporary connector entries for the current session only, without
//! creating a persisted operation. Works on both `Bundle` and `BundleBuilder`
//! via `BundleFacade`.

use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::facade::BundleFacade;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use std::sync::Arc;

/// Command to rename runtime-only connector entries (not persisted).
///
/// Unlike `RenameConnectorCommand` which persists to the bundle,
/// this command only renames entries for the current session. It works
/// on both read-only `Bundle` and `BundleBuilder` via `BundleFacade`.
#[derive(Debug, Clone)]
pub struct RenameTempConnectorCommand {
    /// The current connector name (dotted, e.g. "acme.weather")
    pub old_name: String,
    /// The new connector name (dotted, e.g. "acme.weather_v2")
    pub new_name: String,
}

impl RenameTempConnectorCommand {
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }

    /// Returns the Arrow schema for rename temporary connector output.
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

impl CommandParsing for RenameTempConnectorCommand {
    fn rule() -> Rule {
        Rule::rename_temp_connector_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut old_name = None;
        let mut new_name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::dotted_identifier {
                if old_name.is_none() {
                    old_name = Some(inner_pair.as_str().to_string());
                } else {
                    new_name = Some(inner_pair.as_str().to_string());
                }
            }
        }

        let old_name = old_name.ok_or_else(|| -> BundlebaseError {
            "RENAME TEMP CONNECTOR missing old name".into()
        })?;
        let new_name = new_name.ok_or_else(|| -> BundlebaseError {
            "RENAME TEMP CONNECTOR missing new name".into()
        })?;

        Ok(RenameTempConnectorCommand::new(old_name, new_name))
    }

    fn to_statement(&self) -> String {
        format!(
            "RENAME TEMP CONNECTOR {} TO {}",
            self.old_name, self.new_name
        )
    }
}

impl BundleFacadeCommand for RenameTempConnectorCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        facade
            .rename_temp_connector(&self.old_name, &self.new_name)
            .await?;

        Ok(format!(
            "Renamed temporary connector: {} to {}",
            self.old_name, self.new_name
        ))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_rename_temp_connector() {
        let input = "RENAME TEMP CONNECTOR acme.weather TO acme.weather_v2";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameTempConnector(c) => {
                assert_eq!(c.old_name, "acme.weather");
                assert_eq!(c.new_name, "acme.weather_v2");
            }
            _ => panic!("Expected RenameTempConnector variant"),
        }
    }

    #[test]
    fn test_parse_rename_temp_connector_case_insensitive() {
        let input = "rename temp connector acme.weather to acme.weather_v2";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameTempConnector(c) => {
                assert_eq!(c.old_name, "acme.weather");
                assert_eq!(c.new_name, "acme.weather_v2");
            }
            _ => panic!("Expected RenameTempConnector variant"),
        }
    }

    #[test]
    fn test_parse_rename_temp_connector_roundtrip() {
        let cmd = RenameTempConnectorCommand::new("acme.weather", "acme.weather_v2");
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "RENAME TEMP CONNECTOR acme.weather TO acme.weather_v2"
        );
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RenameTempConnector(c) => {
                assert_eq!(c.old_name, "acme.weather");
                assert_eq!(c.new_name, "acme.weather_v2");
            }
            _ => panic!("Expected RenameTempConnector variant"),
        }
    }
}
