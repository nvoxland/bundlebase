//! DropTempFunction command implementation (runtime-only).
//!
//! Removes function entries for the current session only, without creating a persisted
//! operation. Works on both `Bundle` and `BundleBuilder` via `BundleFacade`.

use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::facade::BundleFacade;
use crate::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use std::sync::Arc;

/// Command to drop runtime-only function entries (not persisted).
///
/// Drops all overloads of the named function.
#[derive(Debug, Clone)]
pub struct DropTempFunctionCommand {
    /// Full dotted function name
    pub name: String,
}

impl DropTempFunctionCommand {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
        }
    }

    /// Returns the Arrow schema for this command's output.
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

impl CommandParsing for DropTempFunctionCommand {
    fn rule() -> Rule {
        Rule::drop_temp_function_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::dotted_identifier {
                name = Some(inner_pair.as_str().to_string());
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DROP TEMP FUNCTION missing function name".into()
        })?;

        Ok(DropTempFunctionCommand::new(name))
    }

    fn to_statement(&self) -> String {
        format!("DROP TEMP FUNCTION {}", self.name)
    }
}

#[async_trait]
impl BundleFacadeCommand for DropTempFunctionCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let count = facade
            .drop_temp_function(&self.name, None)
            .await?;

        Ok(format!(
            "Dropped {} temporary function entries for {}",
            count, self.name
        ))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_temp_function() {
        let input = "DROP TEMP FUNCTION acme.double_val";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropTempFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
            }
            _ => panic!("Expected DropTempFunction variant"),
        }
    }

    #[test]
    fn test_parse_drop_temp_function_roundtrip() {
        let cmd = DropTempFunctionCommand::new("acme.double_val");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP TEMP FUNCTION acme.double_val");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropTempFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
            }
            _ => panic!("Expected DropTempFunction variant"),
        }
    }
}
