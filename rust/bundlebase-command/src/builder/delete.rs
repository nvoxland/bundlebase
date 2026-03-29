//! Delete command implementation.
//!
//! Deletes rows matching a WHERE clause by collecting their RowIds
//! and adding them to the bundle's in-memory deleted set.

use crate::{CommandParsing, Rule};
use bundlebase_common::BundlebaseError;
use crate::BundleBuilderCommand;
use bundlebase::BundleBuilder;
use bundlebase::bundle::column_metadata;
use bundlebase::bundle::operation::FilterOp;
use bundlebase::bundle::BundleFacade;
use tracing::debug;

/// Command to delete rows matching a WHERE condition.
#[derive(Debug, Clone)]
pub struct DeleteCommand {
    /// The WHERE clause condition (without the "WHERE" keyword)
    pub where_clause: String,
}

impl DeleteCommand {
    pub fn new(where_clause: impl Into<String>) -> Self {
        Self {
            where_clause: where_clause.into(),
        }
    }
}

impl CommandParsing for DeleteCommand {
    fn rule() -> Rule {
        Rule::delete_stmt
    }

    fn from_statement(pair: pest::iterators::Pair<Rule>) -> Result<Self, BundlebaseError> {
        let mut where_clause = None;

        for inner_pair in pair.into_inner() {
            if let Rule::delete_where_clause = inner_pair.as_rule() {
                where_clause = Some(inner_pair.as_str().trim().to_string());
            }
        }

        let where_clause = where_clause.ok_or_else(|| -> BundlebaseError {
            "DELETE statement missing WHERE clause".into()
        })?;

        if where_clause.is_empty() {
            return Err("DELETE WHERE clause cannot be empty".into());
        }

        Ok(DeleteCommand::new(where_clause))
    }

    fn to_statement(&self) -> String {
        format!("DELETE FROM bundle WHERE {}", self.where_clause)
    }
}

impl BundleBuilderCommand for DeleteCommand {
    type Output = String;

    async fn execute(self: Box<Self>, builder: &BundleBuilder) -> Result<String, BundlebaseError> {
        // Translate user-visible column names to stable col_<id> references
        let col_names = builder.column_names();
        let where_clause = column_metadata::translate_sql_to_col_ids(&self.where_clause, &col_names);

        // Collect RowIds matching the WHERE clause
        let delete_rowids = builder.select_row_ids(&where_clause).await?;
        let deleted_count = delete_rowids.len();
        debug!("[DELETE] Collected {} RowIds for WHERE {}", deleted_count, where_clause);

        if deleted_count == 0 {
            return Ok("Deleted 0 rows".to_string());
        }

        // Store deleted RowIds for commit
        builder.mark_deleted(delete_rowids, &where_clause);

        // Apply a negated filter to immediately exclude deleted rows from queries
        let filter_query = format!(
            "SELECT * FROM bundle WHERE NOT ({})",
            where_clause
        );
        builder.apply_operation(FilterOp::new(&filter_query, vec![]).into()).await?;

        Ok(format!("Deleted {} rows", deleted_count))
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;
    use crate::parser::parse_command;
    use crate::BundleCommand;

    #[test]
    fn test_parse_delete_simple() {
        let input = "DELETE FROM bundle WHERE salary < 0";
        let cmd = parse_command(input).expect("Failed to parse DELETE");
        match cmd {
            BundleCommand::Delete(c) => {
                assert_eq!(c.where_clause, "salary < 0");
            }
            _ => panic!("Expected Delete variant, got {:?}", cmd),
        }
    }

    #[test]
    fn test_parse_delete_complex_where() {
        let input =
            "DELETE FROM bundle WHERE status = 'inactive' AND last_login < '2020-01-01'";
        let cmd = parse_command(input).expect("Failed to parse DELETE");
        match cmd {
            BundleCommand::Delete(c) => {
                assert_eq!(
                    c.where_clause,
                    "status = 'inactive' AND last_login < '2020-01-01'"
                );
            }
            _ => panic!("Expected Delete variant"),
        }
    }

    #[test]
    fn test_parse_delete_case_insensitive() {
        let input = "delete from bundle where id = 42";
        let cmd = parse_command(input).expect("Failed to parse lowercase DELETE");
        match cmd {
            BundleCommand::Delete(c) => {
                assert_eq!(c.where_clause, "id = 42");
            }
            _ => panic!("Expected Delete variant"),
        }
    }

    #[test]
    fn test_parse_delete_roundtrip() {
        let cmd = DeleteCommand::new("salary < 0");
        let statement = cmd.to_statement();
        assert_eq!(statement, "DELETE FROM bundle WHERE salary < 0");

        let parsed = parse_command(&statement).expect("Failed to re-parse");
        match parsed {
            BundleCommand::Delete(c) => {
                assert_eq!(c.where_clause, "salary < 0");
            }
            _ => panic!("Expected Delete variant"),
        }
    }
}
