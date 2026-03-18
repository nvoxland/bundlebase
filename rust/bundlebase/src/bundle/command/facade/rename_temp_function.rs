//! RenameTempFunction command implementation (runtime-only).
//!
//! Renames temporary function entries for the current session only, without
//! creating a persisted operation. Works on both `Bundle` and `BundleBuilder`
//! via `BundleFacade`.

use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::facade::BundleFacade;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::sync::Arc;

/// Command to rename runtime-only function entries (not persisted).
///
/// Unlike `RenameFunctionCommand` which persists to the bundle,
/// this command only renames entries for the current session. It works
/// on both read-only `Bundle` and `BundleBuilder` via `BundleFacade`.
#[derive(Debug, Clone)]
pub struct RenameTempFunctionCommand {
    /// The current function name (dotted, e.g. "acme.double_val")
    pub old_name: String,
    /// The new function name (dotted, e.g. "acme.double_val_v2")
    pub new_name: String,
}

impl RenameTempFunctionCommand {
    pub fn new(old_name: impl Into<String>, new_name: impl Into<String>) -> Self {
        Self {
            old_name: old_name.into(),
            new_name: new_name.into(),
        }
    }

    /// Returns the Arrow schema for rename temporary function output.
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

impl CommandParsing for RenameTempFunctionCommand {
    fn rule() -> Rule {
        Rule::rename_temp_function_stmt
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
            "RENAME TEMP FUNCTION missing old name".into()
        })?;
        let new_name = new_name.ok_or_else(|| -> BundlebaseError {
            "RENAME TEMP FUNCTION missing new name".into()
        })?;

        Ok(RenameTempFunctionCommand::new(old_name, new_name))
    }

    fn to_statement(&self) -> String {
        format!(
            "RENAME TEMP FUNCTION {} TO {}",
            self.old_name, self.new_name
        )
    }
}

#[async_trait]
impl BundleFacadeCommand for RenameTempFunctionCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        facade
            .rename_temp_function(&self.old_name, &self.new_name)
            .await?;

        Ok(format!(
            "Renamed temporary function: {} to {}",
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
    fn test_parse_rename_temp_function() {
        let input = "RENAME TEMP FUNCTION acme.double_val TO acme.double_val_v2";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameTempFunction(c) => {
                assert_eq!(c.old_name, "acme.double_val");
                assert_eq!(c.new_name, "acme.double_val_v2");
            }
            _ => panic!("Expected RenameTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_rename_temp_function_case_insensitive() {
        let input = "rename temp function acme.double_val to acme.double_val_v2";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::RenameTempFunction(c) => {
                assert_eq!(c.old_name, "acme.double_val");
                assert_eq!(c.new_name, "acme.double_val_v2");
            }
            _ => panic!("Expected RenameTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_rename_temp_function_roundtrip() {
        let cmd = RenameTempFunctionCommand::new("acme.double_val", "acme.double_val_v2");
        let statement = cmd.to_statement();
        assert_eq!(
            statement,
            "RENAME TEMP FUNCTION acme.double_val TO acme.double_val_v2"
        );
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::RenameTempFunction(c) => {
                assert_eq!(c.old_name, "acme.double_val");
                assert_eq!(c.new_name, "acme.double_val_v2");
            }
            _ => panic!("Expected RenameTempFunction variant"),
        }
    }
}
