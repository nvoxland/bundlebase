//! The `mcp` subcommand — MCP server over stdio.
//!
//! Starts without a bundle by default. Agents use the `create_bundle` or
//! `open_bundle` tools to load one. Optionally pass `--bundle` to pre-open.

use super::{open_bundle, BundleArgs};
use bundlebase_common::BundlebaseError;
use clap::Args;

/// Start MCP (Model Context Protocol) server over stdio
#[derive(Args, Debug)]
pub struct McpArgs {
    /// Path or URL to a bundle to pre-open (optional — can also use open_bundle/create_bundle tools)
    #[arg(long)]
    pub bundle: Option<String>,

    /// Open bundle in read-only mode (only with --bundle)
    #[arg(long, default_value = "false")]
    pub read_only: bool,

    /// Path to a YAML or JSON config file
    #[arg(long)]
    pub config: Option<String>,
}

pub async fn run(args: McpArgs) -> Result<(), BundlebaseError> {
    let state = if let Some(ref bundle_path) = args.bundle {
        let bundle_args = BundleArgs {
            bundle: bundle_path.clone(),
            read_only: args.read_only,
            config: args.config,
        };
        Some(open_bundle(&bundle_args).await?)
    } else {
        None
    };

    bundlebase_cli::mcp::start(state).await
}
