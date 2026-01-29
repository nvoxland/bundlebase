//! Details command - displays bundle metadata (id, name, url, version, etc.).

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase::bundle::OutputShape;
use bundlebase::BundleFacade;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Command metadata
pub const DEF: ReplCommandDef = ReplCommandDef {
    name: "details",
    aliases: &[],
    description: "Show bundle details",
    usage: "/details",
    create,
    execute,
};

fn create(_args: &str) -> Result<ReplCommand, String> {
    Ok(ReplCommand::Details)
}

/// Queries bundle_info.details and returns as Dictionary format
fn execute(_cmd: &ReplCommand, bundle: &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult> {
    let bundle = bundle.clone();
    Box::pin(async move {
        let stream = bundle
            .query("SELECT * FROM bundle_info.details", vec![])
            .await?;
        Ok(Some((stream, OutputShape::Dictionary)))
    })
}
