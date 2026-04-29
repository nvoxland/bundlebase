//! The `repl` subcommand — interactive REPL mode.

use super::{open_bundle, BundleArgs};
use bundlebase_cli::OutputFormat;
use bundlebase_common::BundlebaseError;
use clap::Args;
use std::io::{IsTerminal, Read};

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

    // `bundlebase repl --bundle ./b < script.sql` is a familiar idiom
    // (psql, sqlite3, mysql all accept it). Reedline can't enter raw
    // mode on a non-tty stdin and would die with `Os { code: 6, ... }`.
    // Detect that up front and fall through to the same one-shot
    // execution path used by `bundlebase query` — same parser, same
    // multi-statement support, same uncapped output.
    if !std::io::stdin().is_terminal() {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).map_err(|e| {
            BundlebaseError::from(format!("Failed to read SQL from stdin: {}", e))
        })?;
        let trimmed = buf.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        return bundlebase_cli::repl::execute_single(state, trimmed, args.format).await;
    }

    bundlebase_cli::repl::print_header(state.as_ref());
    bundlebase_cli::repl::start(state, args.format).await
}
