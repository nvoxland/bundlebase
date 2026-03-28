//! SHOW command implementations.
//!
//! Each SHOW target is its own command struct, generated via the `show_table_command!` macro
//! for table-backed commands. Non-table commands (like ShowCount) live in their own modules.
//!
//! `SHOW HISTORY` is equivalent to `SELECT * FROM bundle_info.history`.

use crate::response::OutputShape;
use crate::{BundleFacadeCommand, CommandParsing, Rule};
use bundlebase::BundleFacade;
use bundlebase::BUNDLE_INFO_SCHEMA;
use bundlebase_common::BundlebaseError;
use arrow::datatypes::{Schema, SchemaRef};
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Generates a SHOW command struct that queries a bundle_info table.
macro_rules! show_table_command {
    ($name:ident, $rule:ident, $table:expr, $keyword:expr) => {
        #[derive(Debug, Clone)]
        pub struct $name;

        impl $name {
            pub fn output_schema() -> SchemaRef {
                Arc::new(Schema::empty())
            }

            pub fn output_shape() -> OutputShape {
                OutputShape::Table
            }
        }

        impl CommandParsing for $name {
            fn rule() -> Rule {
                Rule::$rule
            }

            fn from_statement(
                _pair: pest::iterators::Pair<Rule>,
            ) -> Result<Self, BundlebaseError> {
                Ok($name)
            }

            fn to_statement(&self) -> String {
                format!("SHOW {}", $keyword)
            }
        }

        impl BundleFacadeCommand for $name {
            type Output = SendableRecordBatchStream;

            async fn execute(
                self: Box<Self>,
                facade: &dyn BundleFacade,
            ) -> Result<SendableRecordBatchStream, BundlebaseError> {
                let sql = format!("SELECT * FROM {}.{}", BUNDLE_INFO_SCHEMA, $table);
                facade.query(&sql, vec![], None).await
            }
        }
    };
}

show_table_command!(ShowDetailsCommand, show_details_stmt, "details", "DETAILS");
show_table_command!(ShowHistoryCommand, show_history_stmt, "history", "HISTORY");
show_table_command!(ShowStatusCommand, show_status_stmt, "status", "STATUS");
show_table_command!(ShowViewsCommand, show_views_stmt, "views", "VIEWS");
show_table_command!(ShowIndexesCommand, show_indexes_stmt, "indexes", "INDEXES");
show_table_command!(ShowPacksCommand, show_packs_stmt, "packs", "PACKS");
show_table_command!(ShowBlocksCommand, show_blocks_stmt, "blocks", "BLOCKS");
show_table_command!(ShowConfigCommand, show_config_stmt, "config", "CONFIG");
show_table_command!(ShowCommandsCommand, show_commands_stmt, "commands", "COMMANDS");
show_table_command!(ShowConnectorsCommand, show_connectors_stmt, "connectors", "CONNECTORS");
show_table_command!(ShowFunctionsCommand, show_functions_stmt, "functions", "FUNCTIONS");
show_table_command!(ShowColumnsCommand, show_columns_stmt, "columns", "COLUMNS");
show_table_command!(ShowAlwaysDeletesCommand, show_always_deletes_stmt, "always_deletes", "ALWAYS DELETES");

#[cfg(test)]
mod tests {
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_show_history() {
        let cmd = parse_command("SHOW HISTORY").expect("Failed to parse SHOW HISTORY");
        assert!(matches!(cmd, BundleCommand::ShowHistory(_)));
    }

    #[test]
    fn test_parse_show_details() {
        let cmd = parse_command("SHOW DETAILS").expect("Failed to parse SHOW DETAILS");
        assert!(matches!(cmd, BundleCommand::ShowDetails(_)));
    }

    #[test]
    fn test_parse_show_status() {
        let cmd = parse_command("SHOW STATUS").expect("Failed to parse SHOW STATUS");
        assert!(matches!(cmd, BundleCommand::ShowStatus(_)));
    }

    #[test]
    fn test_parse_show_columns() {
        let cmd = parse_command("SHOW COLUMNS").expect("Failed to parse SHOW COLUMNS");
        assert!(matches!(cmd, BundleCommand::ShowColumns(_)));
    }

    #[test]
    fn test_parse_show_case_insensitive() {
        assert!(matches!(
            parse_command("show config").unwrap(),
            BundleCommand::ShowConfig(_)
        ));
        assert!(matches!(
            parse_command("Show History").unwrap(),
            BundleCommand::ShowHistory(_)
        ));
    }

    #[test]
    fn test_parse_show_invalid() {
        let result = parse_command("SHOW NONSENSE");
        assert!(result.is_err(), "SHOW NONSENSE should fail to parse");
    }

    #[test]
    fn test_parse_all_show_commands() {
        let cases = vec![
            ("SHOW DETAILS", "ShowDetails"),
            ("SHOW HISTORY", "ShowHistory"),
            ("SHOW STATUS", "ShowStatus"),
            ("SHOW VIEWS", "ShowViews"),
            ("SHOW INDEXES", "ShowIndexes"),
            ("SHOW PACKS", "ShowPacks"),
            ("SHOW BLOCKS", "ShowBlocks"),
            ("SHOW CONFIG", "ShowConfig"),
            ("SHOW COMMANDS", "ShowCommands"),
            ("SHOW CONNECTORS", "ShowConnectors"),
            ("SHOW FUNCTIONS", "ShowFunctions"),
            ("SHOW COLUMNS", "ShowColumns"),
        ];
        for (sql, expected_name) in cases {
            let cmd = parse_command(sql).unwrap_or_else(|e| panic!("Failed to parse {}: {}", sql, e));
            let debug = format!("{:?}", cmd);
            assert!(
                debug.starts_with(expected_name),
                "Expected {} for '{}', got {:?}",
                expected_name,
                sql,
                cmd
            );
        }
    }
}
