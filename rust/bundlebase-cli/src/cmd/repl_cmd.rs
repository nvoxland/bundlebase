//! The `repl` subcommand — interactive REPL mode.

use super::{load_config, open_bundle_with_create_hint, BundleArgs};
use bundlebase::{BundleBuilder, BundleFacade};
use bundlebase_cli::OutputFormat;
use bundlebase_common::BundlebaseError;
use clap::Args;
use std::io::{IsTerminal, Read};
use std::sync::Arc;

/// Interactive REPL mode
#[derive(Args, Debug)]
pub struct ReplArgs {
    #[command(flatten)]
    pub bundle: BundleArgs,

    /// Output format for query results
    #[arg(long, value_enum, default_value = "table")]
    pub format: OutputFormat,

    /// Create a new bundle at the path before opening the REPL.
    /// Errors if a bundle already exists at that path.
    #[arg(long, default_value = "false")]
    pub create: bool,
}

pub async fn run(args: ReplArgs) -> Result<(), BundlebaseError> {
    let state: Arc<dyn BundleFacade> = if args.create {
        if args.bundle.read_only {
            return Err(BundlebaseError::from(
                "--create cannot be combined with --read-only".to_string(),
            ));
        }
        let config = load_config(args.bundle.config.as_deref())?;
        let builder = match BundleBuilder::create(&args.bundle.bundle, config).await {
            Ok(b) => b,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("A bundle already exists") {
                    return Err(format!(
                        "A bundle already exists at '{}'. Drop --create to open it.",
                        args.bundle.bundle
                    )
                    .into());
                }
                return Err(e);
            }
        };
        // Commit an initial empty state so the bundle persists on disk and is
        // re-openable after the REPL exits — `BundleBuilder::create` only
        // reserves the location; the on-disk bundle isn't established until
        // commit.
        builder.commit("Initial commit").await?;
        builder
    } else {
        open_bundle_with_create_hint(
            &args.bundle,
            "Add --create to create a new bundle at this path.",
        )
        .await?
    };

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
