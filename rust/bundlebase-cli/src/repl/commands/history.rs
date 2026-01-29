//! History command - displays the bundle's commit history.

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase::bundle::CommandResponse;
use bundlebase::BundleFacade;
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
            Box::new(commits)
        };
        let (stream, shape) = super::response_to_stream(response.as_ref())?;
        Ok(Some((stream, shape)))
    })
}

#[cfg(test)]
mod tests {
    use bundlebase::bundle::{BundleCommit, CommandResponse};

    #[test]
    fn test_history_result_empty() {
        let commits: Vec<BundleCommit> = vec![];
        let batch = commits.to_record_batch().unwrap();
        assert_eq!(batch.num_rows(), 0);
        assert_eq!(batch.num_columns(), 6);
    }
}
