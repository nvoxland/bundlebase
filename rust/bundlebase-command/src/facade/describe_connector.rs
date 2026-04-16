//! DescribeConnector command implementation (read-only facade).
//!
//! Returns metadata about a registered connector: all entries matching
//! the given name, including runtime, entrypoint, platform, temporary status, and args.
//! Works for both built-in connectors (plain name like `http`) and imported connectors
//! (dotted name like `acme.weather`).

use crate::parser::extract_identifier;
use crate::response::{single_batch_stream, OutputShape};
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use arrow::array::{ArrayRef, BooleanArray, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Command to describe a registered connector's metadata.
///
/// Returns a table with columns: name, runtime, entrypoint, platform, temporary, args.
/// Works for built-in connectors (e.g., `DESCRIBE CONNECTOR http`) and imported
/// connectors (e.g., `DESCRIBE CONNECTOR acme.weather`).
#[derive(Debug, Clone)]
pub struct DescribeConnectorCommand {
    /// Connector name — plain (e.g., "http") or dotted (e.g., "acme.weather")
    pub name: String,
}

impl DescribeConnectorCommand {
    pub fn new(name: impl Into<String>) -> Result<Self, BundlebaseError> {
        Ok(Self { name: name.into() })
    }

    /// Returns the Arrow schema for describe connector output.
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("runtime", DataType::Utf8, false),
            Field::new("entrypoint", DataType::Utf8, false),
            Field::new("platform", DataType::Utf8, false),
            Field::new("temporary", DataType::Boolean, false),
            Field::new("args", DataType::Utf8, true),
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
            match inner_pair.as_rule() {
                Rule::dotted_identifier => {
                    name = Some(inner_pair.as_str().to_string());
                }
                Rule::identifier => {
                    name = Some(extract_identifier(&inner_pair));
                }
                _ => {}
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
        let schema = Self::output_schema();
        let registry = facade.connector_registry();
        let reg = registry.read();

        // Try built-in connector first (plain names like "http", "kaggle", etc.)
        if let Some(builtin) = reg.get(&self.name) {
            let sig = builtin.signature();
            let args_desc: Vec<String> = sig
                .arg_specs
                .iter()
                .map(|s| {
                    if s.required {
                        format!("{} (required)", s.name)
                    } else if let Some(ref default) = s.default {
                        format!("{} (optional, default: {})", s.name, default)
                    } else {
                        format!("{} (optional)", s.name)
                    }
                })
                .collect();
            let args_str = if args_desc.is_empty() {
                None
            } else {
                Some(args_desc.join(", "))
            };

            let batch = RecordBatch::try_new(
                Arc::clone(&schema),
                vec![
                    Arc::new(StringArray::from(vec![self.name.as_str()])) as ArrayRef,
                    Arc::new(StringArray::from(vec!["built-in"])) as ArrayRef,
                    Arc::new(StringArray::from(vec![self.name.as_str()])) as ArrayRef,
                    Arc::new(StringArray::from(vec!["all"])) as ArrayRef,
                    Arc::new(BooleanArray::from(vec![false])) as ArrayRef,
                    Arc::new(StringArray::from(vec![args_str.as_deref()])) as ArrayRef,
                ],
            )
            .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;

            return single_batch_stream(schema, batch);
        }

        // Try imported connector (dotted names like "acme.weather")
        let all_entries = reg.entries().to_vec();
        let matching: Vec<_> = all_entries
            .into_iter()
            .filter(|e| e.name.to_string() == self.name)
            .collect();

        if matching.is_empty() {
            return Err(format!(
                "Connector '{}' not found. Use SHOW CONNECTORS to list available connectors.",
                self.name
            )
            .into());
        }

        let names: Vec<String> = matching.iter().map(|e| e.name.to_string()).collect();
        let runtimes: Vec<String> = matching
            .iter()
            .map(|e| e.from.runtime_name().to_string())
            .collect();
        let entrypoints: Vec<String> = matching
            .iter()
            .map(|e| e.from.to_entrypoint_string())
            .collect();
        let platforms: Vec<String> = matching.iter().map(|e| e.platform.to_string()).collect();
        let temporaries: Vec<bool> = matching.iter().map(|e| e.temporary).collect();
        let args: Vec<Option<&str>> = matching.iter().map(|_| None).collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(names)) as ArrayRef,
                Arc::new(StringArray::from(runtimes)) as ArrayRef,
                Arc::new(StringArray::from(entrypoints)) as ArrayRef,
                Arc::new(StringArray::from(platforms)) as ArrayRef,
                Arc::new(BooleanArray::from(temporaries)) as ArrayRef,
                Arc::new(StringArray::from(args)) as ArrayRef,
            ],
        )
        .map_err(|e| BundlebaseError::from(format!("Failed to create record batch: {}", e)))?;

        single_batch_stream(schema, batch)
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;
    use crate::CommandParsing;

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

    #[test]
    fn test_parse_describe_connector_builtin_plain_name() {
        // Built-in connectors use plain names (no dot) — must not fail with parse error
        let input = "DESCRIBE CONNECTOR http";
        let cmd = parse_command(input).expect("Failed to parse DESCRIBE CONNECTOR with plain name");
        match cmd {
            BundleCommand::DescribeConnector(c) => {
                assert_eq!(c.name, "http");
            }
            _ => panic!("Expected DescribeConnector variant"),
        }
    }

    #[test]
    fn test_parse_describe_connector_builtin_case_insensitive() {
        let input = "describe connector kaggle";
        let cmd = parse_command(input).expect("Failed to parse");
        match cmd {
            BundleCommand::DescribeConnector(c) => {
                assert_eq!(c.name, "kaggle");
            }
            _ => panic!("Expected DescribeConnector variant"),
        }
    }
}
