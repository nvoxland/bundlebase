//! Unified SQL execution utilities for REPL and Flight.
//!
//! This module provides helper functions for SQL execution. The main execution
//! logic is now in `BundleState::execute_sql()`.

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use bundlebase::bundle::{is_command_statement, parse_command, CommandOutput};
use bundlebase::BundlebaseError;

// Re-export SqlResult from state for backwards compatibility
pub use crate::state::SqlResult;

/// Get the schema for a command without executing it.
///
/// This allows clients to know the output schema at parse time,
/// useful for protocols that need to describe result sets upfront.
///
/// # Arguments
///
/// * `sql` - SQL statement to get schema for
///
/// # Returns
///
/// * `Some(SchemaRef)` - If the SQL is a bundlebase command with known schema
/// * `None` - If the SQL is not a bundlebase command or cannot determine schema
pub fn get_command_schema(sql: &str) -> Option<SchemaRef> {
    let sql = sql.trim();

    // Only bundlebase commands have known schemas at parse time.
    // Standard SQL (including SELECT) needs to be planned to determine the schema.
    if !is_command_statement(sql) {
        return None;
    }

    match parse_command(sql) {
        Ok(cmd) => Some(cmd.output_schema()),
        Err(_) => None,
    }
}

/// Convert a CommandOutput to a vector of RecordBatches.
///
/// This is useful for protocols that need to return batches rather than
/// a streaming result.
pub fn command_output_to_batches(output: &CommandOutput) -> Result<Vec<RecordBatch>, BundlebaseError> {
    let batch = output.to_record_batch()?;
    Ok(vec![batch])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_command_schema_filter() {
        let schema = get_command_schema("FILTER WHERE x = 1");
        assert!(schema.is_some());
        let schema = schema.expect("Schema should be present");
        // FILTER returns a message schema
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.field(0).name(), "message");
    }

    #[test]
    fn test_get_command_schema_verify() {
        let schema = get_command_schema("VERIFY DATA");
        assert!(schema.is_some());
        let schema = schema.expect("Schema should be present");
        // VERIFY DATA returns verification schema with 7 columns
        assert_eq!(schema.fields().len(), 7);
    }

    #[test]
    fn test_get_command_schema_standard_sql() {
        // Standard SQL that doesn't start with a bundlebase keyword
        let schema = get_command_schema("INSERT INTO table VALUES (1)");
        assert!(schema.is_none());
    }

    #[test]
    fn test_get_command_schema_select_returns_none() {
        // SELECT statements should return None so Flight SQL will plan the query
        // to determine the actual result schema, rather than using the bundlebase
        // SELECT command schema which just returns ["message"].
        let schema = get_command_schema("SELECT * FROM bundle");
        assert!(schema.is_none());

        let schema = get_command_schema("select col1, col2 from bundle");
        assert!(schema.is_none());

        let schema = get_command_schema("  SELECT * FROM bundle WHERE x > 1  ");
        assert!(schema.is_none());
    }
}
