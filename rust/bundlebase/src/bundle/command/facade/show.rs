//! Show command implementation.
//!
//! Provides a shortcut for querying bundle_info helper tables.
//! `SHOW HISTORY` is equivalent to `SELECT * FROM bundle_info.history`.

use crate::bundle::command::response::OutputShape;
use crate::bundle::command::{BundleFacadeCommand, CommandParsing, Rule};
use crate::bundle::facade::BundleFacade;
use crate::catalog::{tables, BUNDLE_INFO_SCHEMA};
use crate::BundlebaseError;
use arrow::datatypes::{Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Valid table names for the SHOW command.
const VALID_TABLES: &[&str] = &[
    tables::DETAILS,
    tables::HISTORY,
    tables::STATUS,
    tables::VIEWS,
    tables::INDEXES,
    tables::PACKS,
    tables::BLOCKS,
    tables::CONFIG,
    tables::CONNECTORS,
    tables::FUNCTIONS,
];

/// Command to show contents of a bundle_info helper table.
#[derive(Debug, Clone)]
pub struct ShowCommand {
    /// The helper table name (e.g., "history", "status", "details")
    pub table: String,
}

impl ShowCommand {
    /// Returns a placeholder Arrow schema for show output.
    /// The actual schema depends on which table is queried and comes from the stream.
    pub fn output_schema() -> SchemaRef {
        Arc::new(Schema::empty())
    }

    /// Returns the expected output shape.
    pub fn output_shape() -> OutputShape {
        OutputShape::Table
    }
}

impl CommandParsing for ShowCommand {
    fn rule() -> Rule {
        Rule::show_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut table = None;

        for inner_pair in pair.into_inner() {
            if inner_pair.as_rule() == Rule::show_table {
                table = Some(inner_pair.as_str().to_lowercase());
            }
        }

        let table = table.ok_or_else(|| -> BundlebaseError {
            format!(
                "SHOW requires a table name. Valid tables: {}",
                VALID_TABLES.join(", ")
            )
            .into()
        })?;

        Ok(ShowCommand { table })
    }

    fn to_statement(&self) -> String {
        format!("SHOW {}", self.table.to_uppercase())
    }
}

#[async_trait]
impl BundleFacadeCommand for ShowCommand {
    type Output = SendableRecordBatchStream;

    async fn execute(
        self: Box<Self>,
        facade: &dyn BundleFacade,
    ) -> Result<SendableRecordBatchStream, BundlebaseError> {
        let sql = format!("SELECT * FROM {}.{}", BUNDLE_INFO_SCHEMA, self.table);
        facade.query(&sql, vec![], None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::command::parser::parse_command;
    use crate::bundle::command::BundleCommand;

    #[test]
    fn test_parse_show_history() {
        let cmd = parse_command("SHOW HISTORY").expect("Failed to parse SHOW HISTORY");
        match cmd {
            BundleCommand::Show(s) => assert_eq!(s.table, "history"),
            _ => panic!("Expected Show variant"),
        }
    }

    #[test]
    fn test_parse_show_status() {
        let cmd = parse_command("SHOW STATUS").expect("Failed to parse SHOW STATUS");
        match cmd {
            BundleCommand::Show(s) => assert_eq!(s.table, "status"),
            _ => panic!("Expected Show variant"),
        }
    }

    #[test]
    fn test_parse_show_details() {
        let cmd = parse_command("SHOW DETAILS").expect("Failed to parse SHOW DETAILS");
        match cmd {
            BundleCommand::Show(s) => assert_eq!(s.table, "details"),
            _ => panic!("Expected Show variant"),
        }
    }

    #[test]
    fn test_parse_show_case_insensitive() {
        let cmd = parse_command("show config").expect("Failed to parse show config");
        match cmd {
            BundleCommand::Show(s) => assert_eq!(s.table, "config"),
            _ => panic!("Expected Show variant"),
        }
    }

    #[test]
    fn test_parse_all_valid_tables() {
        for table in VALID_TABLES {
            let sql = format!("SHOW {}", table.to_uppercase());
            let cmd = parse_command(&sql).unwrap_or_else(|e| panic!("Failed to parse {}: {}", sql, e));
            match cmd {
                BundleCommand::Show(s) => assert_eq!(s.table, *table),
                _ => panic!("Expected Show variant for {}", table),
            }
        }
    }

    #[test]
    fn test_parse_show_invalid_table() {
        let result = parse_command("SHOW NONSENSE");
        assert!(result.is_err(), "SHOW NONSENSE should fail to parse");
    }

    #[test]
    fn test_roundtrip() {
        let cmd = ShowCommand {
            table: "history".to_string(),
        };
        let stmt = cmd.to_statement();
        assert_eq!(stmt, "SHOW HISTORY");
        let parsed = parse_command(&stmt).expect("Failed to re-parse");
        match parsed {
            BundleCommand::Show(s) => assert_eq!(s.table, "history"),
            _ => panic!("Expected Show variant"),
        }
    }
}
