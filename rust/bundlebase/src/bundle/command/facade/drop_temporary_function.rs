//! DropTemporaryFunction command implementation (runtime-only).
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
#[derive(Debug, Clone)]
pub struct DropTemporaryFunctionCommand {
    /// Full dotted function name
    pub name: String,
    /// Optional input type signature filter
    pub input_types: Option<Vec<String>>,
}

impl DropTemporaryFunctionCommand {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            input_types: None,
        }
    }

    pub fn new_with_signature(name: impl Into<String>, input_types: Option<Vec<String>>) -> Self {
        Self {
            name: name.into(),
            input_types,
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

impl CommandParsing for DropTemporaryFunctionCommand {
    fn rule() -> Rule {
        Rule::drop_temporary_function_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;
        let mut input_types = Vec::new();
        let mut has_type_signature = false;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::function_params => {
                    has_type_signature = true;
                    for param_pair in inner_pair.into_inner() {
                        if param_pair.as_rule() == Rule::identifier {
                            input_types.push(param_pair.as_str().to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DROP TEMPORARY FUNCTION missing function name".into()
        })?;

        let input_types = if has_type_signature {
            Some(input_types)
        } else {
            None
        };

        Ok(DropTemporaryFunctionCommand::new_with_signature(name, input_types))
    }

    fn to_statement(&self) -> String {
        match &self.input_types {
            Some(types) => format!("DROP TEMPORARY FUNCTION {}({})", self.name, types.join(", ")),
            None => format!("DROP TEMPORARY FUNCTION {}", self.name),
        }
    }
}

#[async_trait]
impl BundleFacadeCommand for DropTemporaryFunctionCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let count = facade
            .drop_temporary_function(&self.name, None)
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
    fn test_parse_drop_temporary_function() {
        let input = "DROP TEMPORARY FUNCTION acme.double_val";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropTemporaryFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
            }
            _ => panic!("Expected DropTemporaryFunction variant"),
        }
    }

    #[test]
    fn test_parse_drop_temporary_function_with_types() {
        let input = "DROP TEMPORARY FUNCTION acme.double_val(Int64)";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropTemporaryFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string()]));
            }
            _ => panic!("Expected DropTemporaryFunction variant"),
        }
    }

    #[test]
    fn test_parse_drop_temporary_function_roundtrip() {
        let cmd = DropTemporaryFunctionCommand::new("acme.double_val");
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropTemporaryFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
            }
            _ => panic!("Expected DropTemporaryFunction variant"),
        }
    }

    #[test]
    fn test_parse_drop_temporary_function_with_types_roundtrip() {
        let cmd = DropTemporaryFunctionCommand::new_with_signature(
            "acme.double_val",
            Some(vec!["Int64".to_string()]),
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP TEMPORARY FUNCTION acme.double_val(Int64)");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropTemporaryFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string()]));
            }
            _ => panic!("Expected DropTemporaryFunction variant"),
        }
    }
}
