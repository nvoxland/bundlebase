//! History command - displays the bundle's commit history.

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase_command::CommandResponse;
use bundlebase::BundleFacade;
use bundlebase::bundle::CommitHistory;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Command metadata
pub const DEF: ReplCommandDef = ReplCommandDef {
    name: "history",
    aliases: &[],
    description: "Show commit history",
    usage: "/history",
    create,
    execute,
};

fn create(_args: &str) -> Result<ReplCommand, String> {
    Ok(ReplCommand::History)
}

fn execute(_cmd: &ReplCommand, bundle: &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult> {
    let bundle = bundle.clone();
    Box::pin(async move {
        let commits = bundle.history();
        let response: Box<dyn CommandResponse> = if commits.is_empty() {
            Box::new("No commit history".to_string())
        } else {
            Box::new(CommitHistory::from(commits))
        };
        let (stream, shape) = super::response_to_stream(response)?;
        Ok(Some((stream, shape)))
    })
}

#[cfg(test)]
mod tests {
    use bundlebase::bundle::{BundleCommit, CommitHistory};
    use bundlebase_command::CommandResponse;
    use futures::TryStreamExt;

    #[tokio::test]
    async fn test_history_result_empty() {
        let commits: Vec<BundleCommit> = vec![];
        let stream = Box::new(CommitHistory::from(commits)).into_stream().unwrap();
        let batches: Vec<arrow::array::RecordBatch> = stream.try_collect().await.unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 0);
        assert_eq!(batches[0].num_columns(), 6);
    }
}
