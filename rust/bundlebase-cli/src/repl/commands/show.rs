//! Show command - displays rows from the bundle's data.

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase::bundle::OutputShape;
use bundlebase::BundleFacade;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Command metadata
pub const DEF: ReplCommandDef = ReplCommandDef {
    name: "show",
    aliases: &[],
    description: "Display rows",
    usage: "/show [limit <n>]",
    create,
    execute,
};

fn create(args: &str) -> Result<ReplCommand, String> {
    let limit = args
        .to_uppercase()
        .strip_prefix("LIMIT")
        .and_then(|s| s.trim().parse().ok());
    Ok(ReplCommand::Show { limit })
}

/// Returns a stream directly since it streams DataFrame data
fn execute(cmd: &ReplCommand, bundle: &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult> {
    let bundle = bundle.clone();
    let limit = match cmd {
        ReplCommand::Show { limit } => *limit,
        _ => None,
    };
    Box::pin(async move {
        let df = bundle.dataframe().await?;
        let limited_df = if let Some(n) = limit {
            df.as_ref().clone().limit(0, Some(n))?
        } else {
            df.as_ref().clone()
        };
        let stream = limited_df.execute_stream().await?;
        Ok(Some((stream, OutputShape::Table)))
    })
}
