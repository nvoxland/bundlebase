//! The `mcp` subcommand — MCP server over stdio.

use super::{open_bundle, BundleArgs};
use bundlebase::BundlebaseError;
use clap::Args;

/// Start MCP (Model Context Protocol) server over stdio
#[derive(Args, Debug)]
pub struct McpArgs {
    #[command(flatten)]
    pub bundle: BundleArgs,
}

pub async fn run(args: McpArgs) -> Result<(), BundlebaseError> {
    let state = open_bundle(&args.bundle).await?;
    bundlebase_cli::mcp::start(state).await
}
