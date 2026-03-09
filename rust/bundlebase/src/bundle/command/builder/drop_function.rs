//! DropFunction command implementation (persistent).

use crate::bundle::command::{CommandParsing, Rule};
use crate::bundle::connector_definition::Platform;
use crate::bundle::function_definition::parse_arrow_type_name;
use crate::bundle::operation::DropFunctionOp;
use crate::BundlebaseError;
use arrow::datatypes::DataType;
use async_trait::async_trait;
use super::super::BundleBuilderCommand;
use crate::bundle::BundleBuilder;

/// Command to drop a function definition.
#[derive(Debug, Clone)]
pub struct DropFunctionCommand {
    /// Full dotted function name
    pub name: String,
    /// Optional platform filter
    pub platform: Option<Platform>,
    /// Optional input type signature filter
    pub input_types: Option<Vec<String>>,
}

impl DropFunctionCommand {
    pub fn new(name: impl Into<String>, platform: Option<Platform>) -> Self {
        Self {
            name: name.into(),
            platform,
            input_types: None,
        }
    }

    pub fn new_with_signature(
        name: impl Into<String>,
        platform: Option<Platform>,
        input_types: Option<Vec<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            platform,
            input_types,
        }
    }
}

impl CommandParsing for DropFunctionCommand {
    fn rule() -> Rule {
        Rule::drop_function_stmt
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
            "DROP FUNCTION missing function name".into()
        })?;

        let input_types = if has_type_signature {
            Some(input_types)
        } else {
            None
        };

        Ok(DropFunctionCommand::new_with_signature(name, None, input_types))
    }

    fn to_statement(&self) -> String {
        match &self.input_types {
            Some(types) => format!("DROP FUNCTION {}({})", self.name, types.join(", ")),
            None => format!("DROP FUNCTION {}", self.name),
        }
    }
}

#[async_trait]
impl BundleBuilderCommand for DropFunctionCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        let parsed_types = match &self.input_types {
            Some(type_names) => {
                let types = type_names.iter()
                    .map(|s| parse_arrow_type_name(s))
                    .collect::<Result<Vec<DataType>, _>>()?;
                Some(types)
            }
            None => None,
        };
        let op = DropFunctionOp::new_with_signature(
            self.name.clone(),
            self.platform.clone(),
            parsed_types,
        );
        builder.apply_operation(op.into()).await?;

        Ok(format!("Dropped function: {}", self.name))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_drop_function() {
        let input = "DROP FUNCTION acme.double_val";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
            }
            _ => panic!("Expected DropFunction variant"),
        }
    }

    #[test]
    fn test_parse_drop_function_with_types() {
        let input = "DROP FUNCTION acme.double_val(Int64)";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string()]));
            }
            _ => panic!("Expected DropFunction variant"),
        }
    }

    #[test]
    fn test_parse_drop_function_with_multi_types() {
        let input = "DROP FUNCTION acme.add(Int64, Int64)";
        let cmd = parse_command(input).unwrap();
        match cmd {
            BundleCommand::DropFunction(c) => {
                assert_eq!(c.name, "acme.add");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string(), "Int64".to_string()]));
            }
            _ => panic!("Expected DropFunction variant"),
        }
    }

    #[test]
    fn test_parse_drop_function_roundtrip() {
        let cmd = DropFunctionCommand::new("acme.double_val", None);
        let statement = cmd.to_statement();
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert!(c.input_types.is_none());
            }
            _ => panic!("Expected DropFunction variant"),
        }
    }

    #[test]
    fn test_parse_drop_function_with_types_roundtrip() {
        let cmd = DropFunctionCommand::new_with_signature(
            "acme.double_val",
            None,
            Some(vec!["Int64".to_string()]),
        );
        let statement = cmd.to_statement();
        assert_eq!(statement, "DROP FUNCTION acme.double_val(Int64)");
        let parsed = parse_command(&statement).unwrap();
        match parsed {
            BundleCommand::DropFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
                assert_eq!(c.input_types, Some(vec!["Int64".to_string()]));
            }
            _ => panic!("Expected DropFunction variant"),
        }
    }
}
