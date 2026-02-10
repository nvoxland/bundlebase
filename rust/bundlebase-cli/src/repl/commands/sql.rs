//! SQL command - executes SQL statements and bundlebase commands.

use bundlebase::bundle::OutputShape;
use bundlebase::BundlebaseError;
use bundlebase::BundleFacade;
use datafusion::execution::SendableRecordBatchStream;
use std::sync::Arc;

/// Execute a SQL statement or bundlebase command
pub async fn execute(
    bundle: &Arc<dyn BundleFacade>,
    sql: &str,
) -> Result<(SendableRecordBatchStream, OutputShape), BundlebaseError> {
    // Get output shape for display formatting
    let (_schema, shape) = bundle.response_schema(sql).await?;

    // Execute and get stream
    let stream = bundle.execute(sql, vec![]).await?;

    Ok((stream, shape))
}
