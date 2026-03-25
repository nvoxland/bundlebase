//! SQL command - executes SQL statements and bundlebase commands.

use bundlebase_command::{BundleFacadeCommandExt, OutputShape};
use bundlebase_command::parser::is_command_statement;
use bundlebase_common::BundlebaseError;
use bundlebase::BundleFacade;
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Default hard limit for query results in CLI mode.
pub const CLI_QUERY_LIMIT: usize = 1000;

/// Execute a SQL statement or bundlebase command
pub async fn execute(
    bundle: &Arc<dyn BundleFacade>,
    sql: &str,
) -> Result<(SendableRecordBatchStream, OutputShape), BundlebaseError> {
    // Get output shape for display formatting
    let (_schema, shape) = bundle.response_schema(sql).await?;

    // Execute: commands go through execute_command, SQL queries use query with limit
    let stream = if is_command_statement(sql) {
        bundle.execute(sql, vec![]).await?
    } else {
        bundle.query(sql, vec![], Some(CLI_QUERY_LIMIT)).await?
    };

    Ok((stream, shape))
}
