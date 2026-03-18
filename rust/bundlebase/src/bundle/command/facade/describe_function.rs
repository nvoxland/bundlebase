//! DescribeFunction command implementation (read-only facade).
//!
//! Returns metadata about a registered function: all entries matching
//! the given dotted name, including kind, input types, return type,
//! runtime, entrypoint, platform, and temporary status.

use crate::bundle::command::response::{single_batch_stream, OutputShape};
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::facade::BundleFacade;
use crate::namespaced_name::NamespacedName;
use crate::BundlebaseError;
use arrow::array::{ArrayRef, BooleanArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Command to describe a registered function's metadata.
///
/// Returns a table with columns: name, kind, input_types, return_type,
/// runtime, entrypoint, platform, temporary for all entries matching the given
/// function name.
#[derive(Debug, Clone)]
pub struct DescribeFunctionCommand {
    /// Full dotted function name (e.g., "acme.double_val")
    pub name: NamespacedName,
}

impl DescribeFunctionCommand {
    pub fn new(name: impl Into<String>) -> Result<Self, BundlebaseError> {
        let name_str: String = name.into();
        Ok(Self {
            name: NamespacedName::parse(&name_str, "Function")?,
        })
    }

    /// Returns the Arrow schema for describe function output.
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new("input_types", DataType::Utf8, false),
            Field::new("return_type", DataType::Utf8, false),
            Field::new("runtime", DataType::Utf8, false),
            Field::new("entrypoint", DataType::Utf8, false),
            Field::new("platform", DataType::Utf8, false),
            Field::new("temporary", DataType::Boolean, false),
        ]))
    }

    /// Returns the expected output shape.
    pub fn output_shape() -> OutputShape {
        OutputShape::Table
    }
}

impl CommandParsing for DescribeFunctionCommand {
    fn rule() -> Rule {
        Rule::describe_function_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::dotted_identifier {
                name = Some(inner_pair.as_str().to_string());
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DESCRIBE FUNCTION missing function name".into()
        })?;

        DescribeFunctionCommand::new(name)
    }

    fn to_statement(&self) -> String {
        format!("DESCRIBE FUNCTION {}", self.name)
    }
}

#[async_trait]
impl BundleFacadeCommand for DescribeFunctionCommand {
    type Output = SendableRecordBatchStream;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let all_entries = facade.function_registry().read().entries().to_vec();
        let matching: Vec<_> = all_entries
            .into_iter()
            .filter(|e| e.name == self.name)
            .collect();

        if matching.is_empty() {
            return Err(format!("Function '{}' is not defined", self.name).into());
        }

        let schema = Self::output_schema();

        let names: Vec<String> = matching.iter().map(|e| e.name.to_string()).collect();
        let kinds: Vec<String> = matching.iter().map(|e| e.kind.to_string()).collect();
        let input_types: Vec<String> = matching
            .iter()
            .map(|e| {
                let types: Vec<String> = e.input_types.iter().map(|t| t.to_string()).collect();
                types.join(", ")
            })
            .collect();
        let return_types: Vec<String> = matching.iter().map(|e| e.return_type.to_string()).collect();
        let runtimes: Vec<String> = matching.iter().map(|e| e.from.runtime_name().to_string()).collect();
        let entrypoints: Vec<String> = matching.iter().map(|e| e.from.to_entrypoint_string()).collect();
        let platforms: Vec<String> = matching.iter().map(|e| e.platform.to_string()).collect();
        let temporaries: Vec<bool> = matching.iter().map(|e| e.temporary).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(names)) as ArrayRef,
                Arc::new(StringArray::from(kinds)) as ArrayRef,
                Arc::new(StringArray::from(input_types)) as ArrayRef,
                Arc::new(StringArray::from(return_types)) as ArrayRef,
                Arc::new(StringArray::from(runtimes)) as ArrayRef,
                Arc::new(StringArray::from(entrypoints)) as ArrayRef,
                Arc::new(StringArray::from(platforms)) as ArrayRef,
                Arc::new(BooleanArray::from(temporaries)) as ArrayRef,
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;

        single_batch_stream(schema, batch)
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_describe_function() {
        let input = "DESCRIBE FUNCTION acme.double_val";
        let cmd = parse_command(input).expect("Failed to parse DESCRIBE FUNCTION");
        match cmd {
            BundleCommand::DescribeFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
            }
            _ => panic!("Expected DescribeFunction variant"),
        }
    }

    #[test]
    fn test_parse_describe_function_case_insensitive() {
        let input = "describe function acme.double_val";
        let cmd = parse_command(input).expect("Failed to parse describe function");
        match cmd {
            BundleCommand::DescribeFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
            }
            _ => panic!("Expected DescribeFunction variant"),
        }
    }

    #[test]
    fn test_parse_describe_function_roundtrip() {
        let cmd = DescribeFunctionCommand::new("acme.double_val").unwrap();
        let statement = cmd.to_statement();
        assert_eq!(statement, "DESCRIBE FUNCTION acme.double_val");
        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::DescribeFunction(c) => {
                assert_eq!(c.name, "acme.double_val");
            }
            _ => panic!("Expected DescribeFunction variant"),
        }
    }
}
