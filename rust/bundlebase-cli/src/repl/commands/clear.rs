//! Clear command - clears the terminal screen.

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase::BundleFacade;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Command metadata
pub const DEF: ReplCommandDef = ReplCommandDef {
    name: "clear",
    aliases: &[],
    description: "Clear screen",
    usage: "/clear",
    create,
    execute,
};

fn create(_args: &str) -> Result<ReplCommand, String> {
    Ok(ReplCommand::Clear)
}

fn execute(_cmd: &ReplCommand, _bundle: &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult> {
    print!("\x1B[2J\x1B[1;1H");

    Box::pin(async { Ok(None) })
}
