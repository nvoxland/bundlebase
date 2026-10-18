//! The `create` subcommand — create a new bundle and optionally execute initial commands.
//!
//! Creates a new bundle at the specified path, then optionally executes SQL
//! commands (like ATTACH) and auto-commits.

use super::{auto_commit_message, load_config};
use bundlebase::{Bundle, BundleBuilder, BundleFacade};
use bundlebase_cli::OutputFormat;
use bundlebase_common::BundlebaseError;
use clap::Args;
use std::io::{IsTerminal, Read};
use std::sync::Arc;

/// Create a new bundle, optionally executing initial SQL commands
#[derive(Args, Debug)]
pub struct CreateArgs {
    /// Path or URL for the new bundle
    #[arg(long)]
    pub bundle: String,

    /// SQL or bundlebase command(s) to execute, semicolon-separated (reads from stdin if omitted)
    pub sql: Option<String>,

    /// Output format for query results
    #[arg(long, value_enum, default_value = "table")]
    pub format: OutputFormat,

    /// Commit message (auto-generated from the command if omitted)
    #[arg(long, short = 'm')]
    pub message: Option<String>,

    /// Path to a YAML or JSON config file
    #[arg(long)]
    pub config: Option<String>,

    /// Don't error if a bundle already exists at the path — open and extend it
    /// instead. Makes `create` idempotent, so an agent can run it on every step
    /// of a multi-call workflow without tracking whether the bundle exists yet.
    #[arg(long)]
    pub exists_ok: bool,
}

pub async fn run(args: CreateArgs) -> Result<(), BundlebaseError> {
    let config = load_config(args.config.as_deref())?;
    let state: Arc<dyn BundleFacade> = match BundleBuilder::create(&args.bundle, config.clone())
        .await
    {
        Ok(builder) => builder,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("A bundle already exists") {
                if args.exists_ok {
                    // Create-or-open: the bundle is already there, so open and
                    // extend it in place and apply the command against it.
                    Bundle::open(&args.bundle, config)
                        .await?
                        .extend(None)
                        .await?
                } else {
                    return Err(format!(
                        "A bundle already exists at '{}'. To modify an existing bundle, use 'bundlebase extend' instead, or pass --exists-ok to open it here.",
                        args.bundle
                    ).into());
                }
            } else {
                return Err(e);
            }
        }
    };

    // Read SQL from arg or stdin
    let sql = match args.sql {
        Some(s) => Some(s),
        None => {
            // Only read stdin if it's not a terminal (i.e., data is being piped)
            if std::io::stdin().is_terminal() {
                None
            } else {
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf).map_err(|e| {
                    BundlebaseError::from(format!("Failed to read SQL from stdin: {}", e))
                })?;
                let trimmed = buf.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }
        }
    };

    if let Some(ref sql) = sql {
        bundlebase_cli::repl::execute_single(state.clone(), sql, args.format).await?;
    }

    // Auto-commit if there are uncommitted changes
    if !state.status_changes().is_empty() {
        let message = args.message.unwrap_or_else(|| {
            sql.as_deref()
                .map(auto_commit_message)
                .unwrap_or_else(|| "Created bundle".to_string())
        });
        let commit_sql = format!(
            "COMMIT {}",
            bundlebase_command::parser::escape_string(&message)
        );
        bundlebase_cli::repl::execute_single(state, &commit_sql, OutputFormat::Table).await?;
    }

    Ok(())
}
