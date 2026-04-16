//! The `repl` subcommand — interactive REPL mode.

use super::{open_bundle, BundleArgs};
use bundlebase_cli::OutputFormat;
use bundlebase_common::BundlebaseError;
use clap::Args;

/// Interactive REPL mode
#[derive(Args, Debug)]
pub struct ReplArgs {
    #[command(flatten)]
    pub bundle: BundleArgs,

    /// Output format for query results
    #[arg(long, value_enum, default_value = "table")]
    pub format: OutputFormat,
}

pub async fn run(args: ReplArgs) -> Result<(), BundlebaseError> {
    let state = open_bundle(&args.bundle).await?;
    bundlebase_cli::repl::print_header(state.as_ref());
    bundlebase_cli::repl::start(state, args.format).await
}
