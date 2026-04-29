//! SQL command - executes SQL statements and bundlebase commands.

use bundlebase::BundleFacade;
use bundlebase_command::parser::is_command_statement;
use bundlebase_command::{BundleFacadeCommandExt, OutputShape};
use bundlebase_common::BundlebaseError;
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Default hard limit for SELECT queries in the *interactive* REPL —
/// stops `SELECT * FROM bundle` from dumping millions of rows into a
/// terminal. The one-shot `bundlebase query` / `bundlebase extend` paths
/// use [`execute_with_hard_limit`] with `None` so scripted callers get
/// every row their SQL asked for.
pub const CLI_QUERY_LIMIT: usize = 1000;

/// Execute a SQL statement or bundlebase command. Convenience wrapper
/// around [`execute_with_hard_limit`] that applies the interactive REPL
/// cap to SELECT queries.
pub async fn execute(
    bundle: &Arc<dyn BundleFacade>,
    sql: &str,
) -> Result<(SendableRecordBatchStream, OutputShape), BundlebaseError> {
    execute_with_hard_limit(bundle, sql, Some(CLI_QUERY_LIMIT)).await
}

/// Execute a SQL statement or bundlebase command with an explicit hard
/// row limit (or `None` for unlimited). Commands (UPDATE / FILTER /
/// COMMIT / etc.) ignore the limit; only SELECTs honour it.
pub async fn execute_with_hard_limit(
    bundle: &Arc<dyn BundleFacade>,
    sql: &str,
    hard_limit: Option<usize>,
) -> Result<(SendableRecordBatchStream, OutputShape), BundlebaseError> {
    // Get output shape for display formatting
    let (_schema, shape) = bundle.response_schema(sql).await?;

    // Execute: commands go through execute_command, SQL queries use query with limit
    let stream = if is_command_statement(sql) {
        bundle.execute(sql, vec![]).await?
    } else {
        bundle.query(sql, vec![], hard_limit).await?
    };

    Ok((stream, shape))
}
