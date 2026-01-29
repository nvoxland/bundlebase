//! Count command - displays the number of rows in the bundle.

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase::BundleFacade;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Command metadata
pub const DEF: ReplCommandDef = ReplCommandDef {
    name: "count",
    aliases: &[],
    description: "Show row count",
    usage: "/count",
    create,
    execute,
};

fn create(_args: &str) -> Result<ReplCommand, String> {
    Ok(ReplCommand::Count)
}

fn execute(_cmd: &ReplCommand, bundle: &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult> {
    let bundle = bundle.clone();
    Box::pin(async move {
        let count = bundle.num_rows().await?;
        let (stream, shape) = super::response_to_stream(&count)?;
        Ok(Some((stream, shape)))
    })
}
