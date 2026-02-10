//! Status command - displays uncommitted changes in the bundle.

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase::bundle::CommandResponse;
use bundlebase::BundleFacade;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Command metadata
pub const DEF: ReplCommandDef = ReplCommandDef {
    name: "status",
    aliases: &[],
    description: "Show uncommitted changes",
    usage: "/status",
    create,
    execute,
};

fn create(_args: &str) -> Result<ReplCommand, String> {
    Ok(ReplCommand::Status)
}

fn execute(_cmd: &ReplCommand, bundle: &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult> {
    let bundle = bundle.clone();
    Box::pin(async move {
        let status = bundle.status();
        let response: Box<dyn CommandResponse> = if status.is_empty() {
            Box::new("No uncommitted changes".to_string())
        } else {
            Box::new(status)
        };
        let (stream, shape) = super::response_to_stream(response)?;
        Ok(Some((stream, shape)))
    })
}

#[cfg(test)]
mod tests {
    use bundlebase::bundle::{BundleStatus, CommandResponse};
    use futures::TryStreamExt;

    #[tokio::test]
    async fn test_status_result() {
        let status = BundleStatus::new();
        let stream = Box::new(status).into_stream().unwrap();
        let batches: Vec<arrow::array::RecordBatch> = stream.try_collect().await.unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 0);
        assert_eq!(batches[0].num_columns(), 4);
    }
}
