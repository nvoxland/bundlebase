//! Export Data command implementation.
//!
//! ExportDataCommand is a facade command that executes a SQL query and writes
//! the results to a file in the format determined by the file extension.

use crate::parser::extract_string_content;
use crate::response::OutputShape;
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use bundlebase::bundle::export::create_export_writer;
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use futures::StreamExt;
use std::sync::Arc;

/// Command to export query results to a file.
///
/// The file format is determined by the extension of the output path:
/// - `.csv` - Comma-separated values
/// - `.jsonl` - JSON Lines (one JSON object per line)
#[derive(Debug, Clone)]
pub struct ExportDataCommand {
    pub path: String,
    pub sql: String,
}

impl ExportDataCommand {
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(
            "message",
            DataType::Utf8,
            false,
        )]))
    }

    pub fn output_shape() -> OutputShape {
        OutputShape::SingleValue
    }
}

impl CommandParsing for ExportDataCommand {
    fn rule() -> Rule {
        Rule::export_data_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut path = None;
        let mut sql = None;

        for inner in pair.into_inner() {
            match inner.as_rule() {
                Rule::quoted_string => {
                    path = Some(extract_string_content(inner.as_str())?);
                }
                Rule::export_data_sql => {
                    sql = Some(inner.as_str().trim().to_string());
                }
                _ => {}
            }
        }

        let path = path.ok_or_else(|| BundlebaseError::from("EXPORT DATA TO: missing file path"))?;
        let sql = sql.ok_or_else(|| BundlebaseError::from("EXPORT DATA TO: missing SQL query"))?;

        Ok(ExportDataCommand { path, sql })
    }

    fn to_statement(&self) -> String {
        format!("EXPORT DATA TO '{}' {}", self.path, self.sql)
    }
}

impl BundleFacadeCommand for ExportDataCommand {
    type Output = String;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<String, BundlebaseError> {
        let mut stream = facade.query(&self.sql, vec![], None).await?;
        let schema = {
            use datafusion::physical_plan::RecordBatchStream;
            RecordBatchStream::schema(stream.as_ref().get_ref())
        };

        let mut writer = create_export_writer(&self.path, &schema)?;

        while let Some(batch_result) = stream.next().await {
            let batch = batch_result.map_err(|e| {
                BundlebaseError::from(format!("Failed to read query results: {}", e))
            })?;
            writer.write_batch(&batch)?;
        }

        let row_count = writer.finish()?;
        Ok(format!("Exported {} rows to '{}'", row_count, self.path))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::CommandParsing;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_export_data_basic() {
        let cmd = parse_command("EXPORT DATA TO 'output.csv' SELECT * FROM bundle")
            .expect("Failed to parse EXPORT DATA");
        match cmd {
            BundleCommand::ExportData(ref c) => {
                assert_eq!(c.path, "output.csv");
                assert_eq!(c.sql, "SELECT * FROM bundle");
            }
            _ => panic!("Expected ExportData variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_export_data_with_where() {
        let cmd = parse_command(
            "EXPORT DATA TO '/tmp/results.json' SELECT name, count FROM bundle WHERE active = true",
        )
        .expect("Failed to parse EXPORT DATA");
        match cmd {
            BundleCommand::ExportData(ref c) => {
                assert_eq!(c.path, "/tmp/results.json");
                assert_eq!(
                    c.sql,
                    "SELECT name, count FROM bundle WHERE active = true"
                );
            }
            _ => panic!("Expected ExportData variant"),
        }
    }

    #[test]
    fn test_parse_export_data_case_insensitive() {
        let cmd = parse_command("export data to 'data.table' select * from bundle")
            .expect("Failed to parse case-insensitive EXPORT DATA");
        match cmd {
            BundleCommand::ExportData(ref c) => {
                assert_eq!(c.path, "data.table");
            }
            _ => panic!("Expected ExportData variant"),
        }
    }

    #[test]
    fn test_parse_export_data_double_quoted_path() {
        let cmd = parse_command("EXPORT DATA TO \"output.csv\" SELECT * FROM bundle")
            .expect("Failed to parse EXPORT DATA with double quotes");
        match cmd {
            BundleCommand::ExportData(ref c) => {
                assert_eq!(c.path, "output.csv");
            }
            _ => panic!("Expected ExportData variant"),
        }
    }

    #[test]
    fn test_round_trip() {
        let cmd = ExportDataCommand {
            path: "output.csv".to_string(),
            sql: "SELECT * FROM bundle".to_string(),
        };
        let statement = cmd.to_statement();
        assert_eq!(statement, "EXPORT DATA TO 'output.csv' SELECT * FROM bundle");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::ExportData(ref c) => {
                assert_eq!(c.path, "output.csv");
                assert_eq!(c.sql, "SELECT * FROM bundle");
            }
            _ => panic!("Expected ExportData variant"),
        }
    }
}
