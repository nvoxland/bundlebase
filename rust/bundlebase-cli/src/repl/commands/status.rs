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
        let (stream, shape) = super::response_to_stream(response.as_ref())?;
        Ok(Some((stream, shape)))
    })
}

#[cfg(test)]
mod tests {
    use bundlebase::bundle::{BundleStatus, CommandResponse};

    #[test]
    fn test_status_result() {
        let status = BundleStatus::new();
        let batch = status.to_record_batch().unwrap();
        assert_eq!(batch.num_rows(), 0);
        assert_eq!(batch.num_columns(), 4);
    }
}
