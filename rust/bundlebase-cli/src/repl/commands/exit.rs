//! Exit command - exits the REPL.

use super::{ReplCommandResult, ReplCommand, ReplCommandDef};
use bundlebase::BundleFacade;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Command metadata
pub const DEF: ReplCommandDef = ReplCommandDef {
    name: "exit",
    aliases: &["quit"],
    description: "Exit REPL",
    usage: "/exit",
    create,
    execute,
};

fn create(_args: &str) -> Result<ReplCommand, String> {
    Ok(ReplCommand::Exit)
}

fn execute(_cmd: &ReplCommand, _bundle: &Arc<dyn BundleFacade>) -> BoxFuture<'static, ReplCommandResult> {
    Box::pin(async { Ok(None) })
}
