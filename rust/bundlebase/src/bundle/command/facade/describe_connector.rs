//! DescribeConnector command implementation (read-only facade).
//!
//! Returns metadata about a registered connector: all entries matching
//! the given dotted name, including runtime, entrypoint, platform, and temporary status.

use crate::bundle::command::response::{single_batch_stream, OutputShape};
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::facade::BundleFacade;
use crate::namespaced_name::NamespacedName;
use crate::BundlebaseError;
use arrow::array::{ArrayRef, BooleanArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Command to describe a registered connector's metadata.
///
/// Returns a table with columns: name, runtime, entrypoint, platform, temporary
/// for all entries matching the given connector name.
#[derive(Debug, Clone)]
pub struct DescribeConnectorCommand {
    /// Full dotted connector name (e.g., "acme.weather")
    pub name: NamespacedName,
}

impl DescribeConnectorCommand {
    pub fn new(name: impl Into<String>) -> Result<Self, BundlebaseError> {
        let name_str: String = name.into();
        Ok(Self {
            name: NamespacedName::parse(&name_str, "Connector")?,
        })
    }

    /// Returns the Arrow schema for describe connector output.
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
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

impl CommandParsing for DescribeConnectorCommand {
    fn rule() -> Rule {
        Rule::describe_connector_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::dotted_identifier {
                name = Some(inner_pair.as_str().to_string());
            }
        }

        let name = name.ok_or_else(|| -> BundlebaseError {
            "DESCRIBE CONNECTOR missing connector name".into()
        })?;

        DescribeConnectorCommand::new(name)
    }

    fn to_statement(&self) -> String {
        format!("DESCRIBE CONNECTOR {}", self.name)
    }
}

impl BundleFacadeCommand for DescribeConnectorCommand {
    type Output = SendableRecordBatchStream;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let all_entries = facade.connector_registry().read().entries().to_vec();
        let matching: Vec<_> = all_entries
            .into_iter()
            .filter(|e| e.name == self.name)
            .collect();

        if matching.is_empty() {
            return Err(format!("Connector '{}' is not defined", self.name).into());
        }

        let schema = Self::output_schema();

        let names: Vec<String> = matching.iter().map(|e| e.name.to_string()).collect();
        let runtimes: Vec<String> = matching.iter().map(|e| e.from.runtime_name().to_string()).collect();
        let entrypoints: Vec<String> = matching.iter().map(|e| e.from.to_entrypoint_string()).collect();
        let platforms: Vec<String> = matching.iter().map(|e| e.platform.to_string()).collect();
        let temporaries: Vec<bool> = matching.iter().map(|e| e.temporary).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(names)) as ArrayRef,
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
    fn test_parse_describe_connector() {
        let input = "DESCRIBE CONNECTOR acme.weather";
        let cmd = parse_command(input).expect("Failed to parse DESCRIBE CONNECTOR");
        match cmd {
            BundleCommand::DescribeConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected DescribeConnector variant"),
        }
    }

    #[test]
    fn test_parse_describe_connector_case_insensitive() {
        let input = "describe connector acme.weather";
        let cmd = parse_command(input).expect("Failed to parse describe connector");
        match cmd {
            BundleCommand::DescribeConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected DescribeConnector variant"),
        }
    }

    #[test]
    fn test_parse_describe_connector_roundtrip() {
        let cmd = DescribeConnectorCommand::new("acme.weather").unwrap();
        let statement = cmd.to_statement();
        assert_eq!(statement, "DESCRIBE CONNECTOR acme.weather");
        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::DescribeConnector(c) => {
                assert_eq!(c.name, "acme.weather");
            }
            _ => panic!("Expected DescribeConnector variant"),
        }
    }
}
