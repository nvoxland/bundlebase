//! TEST CONNECTOR command — validates a connector integration without creating a source.
//!
//! Calls discover() to find locations, then data() on the first location to
//! verify schema and sample data. No source or blocks are created.

use crate::parser::{extract_identifier, extract_string_content};
use crate::response::{single_batch_stream, OutputShape};
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use bundlebase::BundleFacade;
use bundlebase_common::connector::{Connector, SourceData};
use bundlebase_common::BundlebaseError;
use arrow::array::{ArrayRef, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::execution::SendableRecordBatchStream;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Command to test a connector without creating a source.
#[derive(Debug, Clone)]
pub struct TestConnectorCommand {
    /// Connector name (for already-imported connectors)
    pub name: Option<String>,
    /// Temp connector path for inline testing (runtime::entrypoint), used with TEST TEMP CONNECTOR
    pub temp: Option<String>,
    /// Connector arguments
    pub args: HashMap<String, String>,
}

impl TestConnectorCommand {
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("section", DataType::Utf8, false),
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, true),
        ]))
    }

    pub fn output_shape() -> OutputShape {
        OutputShape::Table
    }
}

impl CommandParsing for TestConnectorCommand {
    fn rule() -> Rule {
        Rule::test_connector_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut name: Option<String> = None;
        let mut from: Option<String> = None;
        let mut args = HashMap::new();
        let mut identifiers = Vec::new();
        let mut has_dotted = false;
        let mut dotted_name = None;

        for inner_pair in pair.into_inner() {
            match inner_pair.as_rule() {
                Rule::quoted_string => {
                    // This is the TEST TEMP CONNECTOR '<runtime>::<entrypoint>' path
                    from = Some(extract_string_content(inner_pair.as_str())?);
                }
                Rule::dotted_identifier => {
                    dotted_name = Some(inner_pair.as_str().to_string());
                    has_dotted = true;
                }
                Rule::identifier => {
                    identifiers.push(extract_identifier(&inner_pair));
                }
                Rule::source_args => {
                    for arg_pair in inner_pair.into_inner() {
                        if arg_pair.as_rule() == Rule::source_arg_pair {
                            let mut key = None;
                            let mut value = None;
                            for part in arg_pair.into_inner() {
                                match part.as_rule() {
                                    Rule::identifier => {
                                        key = Some(extract_identifier(&part));
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

        if from.is_some() {
            // Temp mode: TEST TEMP CONNECTOR '<runtime>::<entrypoint>' [WITH ...]
            Ok(TestConnectorCommand { name: None, temp: from, args })
        } else {
            // Name mode: TEST CONNECTOR <name> [WITH ...]
            let connector_name = if has_dotted {
                dotted_name
            } else {
                identifiers.into_iter().next()
            };
            Ok(TestConnectorCommand {
                name: connector_name,
                temp: None,
                args,
            })
        }
    }

    fn to_statement(&self) -> String {
        use crate::parser::{escape_string, quote_identifier};

        if let Some(ref from) = self.temp {
            if self.args.is_empty() {
                return format!("TEST TEMP CONNECTOR {}", escape_string(from));
            }
            let mut args_str: Vec<String> = self.args.iter()
                .map(|(k, v)| format!("{} = {}", quote_identifier(k), escape_string(v)))
                .collect();
            args_str.sort();
            return format!("TEST TEMP CONNECTOR {} WITH ({})", escape_string(from), args_str.join(", "));
        }

        let name = self.name.as_deref().unwrap_or("?");
        if self.args.is_empty() {
            format!("TEST CONNECTOR {}", name)
        } else {
            let mut args_str: Vec<String> = self.args.iter()
                .map(|(k, v)| format!("{} = {}", quote_identifier(k), escape_string(v)))
                .collect();
            args_str.sort();
            format!("TEST CONNECTOR {} WITH ({})", name, args_str.join(", "))
        }
    }
}

impl BundleFacadeCommand for TestConnectorCommand {
    type Output = SendableRecordBatchStream;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let start = Instant::now();
        let schema = Self::output_schema();
        let mut sections = Vec::new();
        let mut keys = Vec::new();
        let mut values = Vec::new();

        // Resolve the connector
        let (func, resolved_args): (Arc<dyn Connector>, HashMap<String, String>) =
            if let Some(ref from_str) = self.temp {
                // TEST TEMP CONNECTOR mode — parse runtime and create inline instance
                let udf_runtime = bundlebase_udf::UdfRuntime::parse_from(from_str)?;
                let runtime_type = udf_runtime.runtime_type();
                let registry = facade.connector_registry();
                let reg = registry.read();
                let func = reg.create_instance(runtime_type)
                    .ok_or_else(|| format!("Unknown connector runtime in '{}'", from_str))?;

                let mut merged = self.args.clone();
                merged.insert("call".to_string(), from_str.clone());
                (func, merged)
            } else if let Some(ref name) = self.name {
                if name.contains('.') {
                    // Custom connector — resolve from registry
                    let registry = facade.connector_registry();
                    let reg = registry.read();
                    let entry = reg.resolve_entry(name)?;
                    let runtime_type = entry.from.runtime_type();
                    let func = reg.create_instance(runtime_type)
                        .ok_or_else(|| format!("Unknown connector type for '{}'", name))?;
                    let resolved_from = entry.from.resolve_path(&facade.data_dir());
                    let mut merged = self.args.clone();
                    merged.insert("call".to_string(), resolved_from.build_call_string());
                    (func, merged)
                } else {
                    // Built-in connector
                    let registry = facade.connector_registry();
                    let reg = registry.read();
                    let func = reg.get(name)
                        .ok_or_else(|| format!("Unknown connector '{}'", name))?;
                    (func, self.args.clone())
                }
            } else {
                return Err("TEST CONNECTOR requires a connector name; use TEST TEMP CONNECTOR for inline testing".into());
            };

        // Step 1: Discover
        let discover_start = Instant::now();
        let config: Arc<dyn bundlebase_common::config::ConfigProvider> = facade.config();
        let attached: HashSet<String> = HashSet::new();
        let locations = func.discover(&resolved_args, &attached, &config).await?;
        let discover_ms = discover_start.elapsed().as_millis();

        sections.push("discover".to_string());
        keys.push("locations_found".to_string());
        values.push(Some(locations.len().to_string()));

        sections.push("discover".to_string());
        keys.push("time_ms".to_string());
        values.push(Some(discover_ms.to_string()));

        // Show first 5 discovered locations
        for (i, loc) in locations.iter().take(5).enumerate() {
            sections.push("discover".to_string());
            keys.push(format!("location_{}", i));
            values.push(Some(format!("{} (format: {}, version: {})",
                loc.location, loc.format, loc.version)));
        }

        if locations.is_empty() {
            sections.push("result".to_string());
            keys.push("status".to_string());
            values.push(Some("No locations discovered — connector returned empty list".to_string()));
        } else {
            // Step 2: Fetch data from first location
            let data_start = Instant::now();
            let first_loc = &locations[0];

            if let Some(source_data) = func.data(first_loc, &resolved_args, &config).await? {
                match source_data {
                    SourceData::Arrow(mut batch_stream) => {
                        // Collect first batch for schema + sample
                        if let Some(batch_result) = batch_stream.next().await {
                            let batch = batch_result?;
                            let data_ms = data_start.elapsed().as_millis();

                            sections.push("data".to_string());
                            keys.push("time_ms".to_string());
                            values.push(Some(data_ms.to_string()));

                            // Schema
                            for field in batch.schema().fields() {
                                sections.push("schema".to_string());
                                keys.push(field.name().clone());
                                values.push(Some(field.data_type().to_string()));
                            }

                            // Row count in sample
                            sections.push("sample".to_string());
                            keys.push("rows_in_batch".to_string());
                            values.push(Some(batch.num_rows().to_string()));
                        }
                    }
                    SourceData::RawBytes(_) => {
                        sections.push("data".to_string());
                        keys.push("type".to_string());
                        values.push(Some("raw_bytes (will be saved as file)".to_string()));
                    }
                }
            } else if let Some(url) = func.stable_url(first_loc, &resolved_args, &config).await? {
                sections.push("data".to_string());
                keys.push("stable_url".to_string());
                values.push(Some(url.to_string()));
            } else {
                sections.push("data".to_string());
                keys.push("error".to_string());
                values.push(Some("Connector returned neither data nor stable_url".to_string()));
            }

            // Result
            let total_ms = start.elapsed().as_millis();
            sections.push("result".to_string());
            keys.push("status".to_string());
            values.push(Some(format!("Connector test passed ({}ms)", total_ms)));
        }

        let values_array: Vec<Option<&str>> = values.iter()
            .map(|v| v.as_deref())
            .collect();

        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(sections.iter().map(|s| s.as_str()).collect::<Vec<_>>())) as ArrayRef,
                Arc::new(StringArray::from(keys.iter().map(|s| s.as_str()).collect::<Vec<_>>())) as ArrayRef,
                Arc::new(StringArray::from(values_array)) as ArrayRef,
            ],
        )?;

        single_batch_stream(schema, batch)
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_test_connector_by_name() {
        let cmd = parse_command("TEST CONNECTOR http WITH (url = 'https://example.com/data.csv')").unwrap();
        match cmd {
            BundleCommand::TestConnector(c) => {
                assert_eq!(c.name, Some("http".to_string()));
                assert!(c.temp.is_none());
                assert_eq!(c.args.get("url"), Some(&"https://example.com/data.csv".to_string()));
            }
            _ => panic!("Expected TestConnector"),
        }
    }

    #[test]
    fn test_parse_test_temp_connector() {
        let cmd = parse_command("TEST TEMP CONNECTOR 'ipc::./my-connector'").unwrap();
        match cmd {
            BundleCommand::TestConnector(c) => {
                assert!(c.name.is_none());
                assert_eq!(c.temp, Some("ipc::./my-connector".to_string()));
            }
            _ => panic!("Expected TestConnector"),
        }
    }

    #[test]
    fn test_parse_test_connector_dotted_name() {
        let cmd = parse_command("TEST CONNECTOR acme.weather WITH (region = 'us-east')").unwrap();
        match cmd {
            BundleCommand::TestConnector(c) => {
                assert_eq!(c.name, Some("acme.weather".to_string()));
                assert_eq!(c.args.get("region"), Some(&"us-east".to_string()));
            }
            _ => panic!("Expected TestConnector"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = TestConnectorCommand {
            name: Some("http".to_string()),
            temp: None,
            args: {
                let mut m = HashMap::new();
                m.insert("url".to_string(), "https://example.com/data.csv".to_string());
                m
            },
        };
        let stmt = cmd.to_statement();
        assert_eq!(stmt, "TEST CONNECTOR http WITH (url = 'https://example.com/data.csv')");
    }
}
