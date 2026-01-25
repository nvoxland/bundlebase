//! Unified SQL execution for REPL and Flight.
//!
//! This module provides a central entry point for SQL execution that handles
//! both bundlebase commands and standard SQL queries, returning appropriate
//! result types for each.

use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use bundlebase::bundle::{
    is_command_statement, parse_command, BundleFacade, CommandOutput,
};
use bundlebase::BundlebaseError;
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

use crate::state::BundleState;

/// Result of SQL execution.
///
/// Distinguishes between streaming query results (SELECT) and
/// command outputs (BundleCommands like ATTACH, FILTER, etc.).
pub enum SqlResult {
    /// Streaming result from a SELECT query.
    Stream(SendableRecordBatchStream),
    /// Command output from a BundleCommand.
    Output(CommandOutput),
}

/// Execute SQL against the bundle state.
///
/// This function handles both bundlebase commands (ATTACH, FILTER, etc.)
/// and standard SQL queries (SELECT). For bundlebase commands, it parses
/// and executes them returning a `CommandOutput`. For SELECT queries
/// (that are not bundlebase SELECTs), it executes via DataFusion and
/// returns a streaming result.
///
/// # Arguments
///
/// * `state` - Shared bundle state
/// * `sql` - SQL statement to execute
///
/// # Returns
///
/// * `Ok(SqlResult::Stream(_))` - For SELECT queries that stream data
/// * `Ok(SqlResult::Output(_))` - For bundlebase commands
/// * `Err(BundlebaseError)` - On execution failure
pub async fn execute_sql(state: &Arc<BundleState>, sql: &str) -> Result<SqlResult, BundlebaseError> {
    let sql = sql.trim();

    // Check if this is a bundlebase command (ATTACH, FILTER, etc. - but NOT SELECT)
    if is_command_statement(sql) {
        // Try to parse as a bundlebase command
        match parse_command(sql) {
            Ok(cmd) => {
                // Clone-modify-writeback pattern: This is safe because each Flight
                // connection has its own BundleState instance, and gRPC serializes
                // requests per connection. We cannot hold the RwLock guard across
                // the await point (not Send).
                let mut builder = {
                    let guard = state.bundle.read();
                    guard.clone()
                };
                let output = cmd.execute(&mut builder).await?;

                // Update the state with the modified builder
                {
                    let mut guard = state.bundle.write();
                    *guard = builder;
                }

                Ok(SqlResult::Output(output))
            }
            Err(e) => {
                // If parsing fails, it might be standard SQL that happens to start
                // with one of our keywords. Try as standard SQL.
                let err_msg = e.to_string();
                if err_msg.contains("Syntax error") {
                    // Could be standard SQL, try executing directly
                    execute_standard_sql(state, sql).await
                } else {
                    Err(e)
                }
            }
        }
    } else {
        // Standard SQL (including SELECT) - execute directly via DataFusion
        execute_standard_sql(state, sql).await
    }
}

/// Execute standard SQL via DataFusion.
///
/// This function handles two cases:
/// 1. Bundle-referencing queries (FROM bundle, JOIN bundle): Uses `builder.select()` to
///    ensure all operations are applied to the dataframe before the query is executed.
/// 2. Non-bundle queries (SELECT 1, etc.): Uses `ctx.sql()` directly on the SessionContext.
///
/// NOTE: We cannot use `ctx.sql()` for bundle queries because the SessionContext's
/// schema provider holds a reference to the original Bundle's DataFrameHolder, not the
/// cloned builder's. Operations applied to the clone (ATTACH, FILTER, etc.) are not
/// visible to the schema provider. The `builder.select()` path explicitly builds the
/// dataframe with all operations applied before executing the query.
async fn execute_standard_sql(
    state: &Arc<BundleState>,
    sql: &str,
) -> Result<SqlResult, BundlebaseError> {
    // Clone the builder to avoid holding the lock during query execution
    let builder = {
        let guard = state.bundle.read();
        guard.clone()
    };

    // Check if the query references the bundle table.
    let sql_upper = sql.to_uppercase();
    if sql_upper.contains("FROM BUNDLE") || sql_upper.contains("JOIN BUNDLE") {
        // Use builder.select() for bundle-referencing queries to get the
        // up-to-date dataframe with all operations applied.
        let result_builder = builder.select(sql, vec![]).await?;
        let df = result_builder.dataframe().await?;
        let stream = df.as_ref().clone().execute_stream().await?;
        Ok(SqlResult::Stream(stream))
    } else {
        // Execute directly via the SessionContext for non-bundle queries
        let ctx = builder.bundle().ctx();
        let df = ctx.sql(sql).await?;
        let stream = df.execute_stream().await?;
        Ok(SqlResult::Stream(stream))
    }
}

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
