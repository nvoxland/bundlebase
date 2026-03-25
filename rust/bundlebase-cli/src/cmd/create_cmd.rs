//! The `create` subcommand — create a new bundle and optionally execute initial commands.
//!
//! Creates a new bundle at the specified path, then optionally executes SQL
//! commands (like ATTACH) and auto-commits.

use super::{auto_commit_message, load_config};
use bundlebase::{BundleBuilder, BundleFacade};
use bundlebase_common::BundlebaseError;
use bundlebase_cli::OutputFormat;
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
}


pub async fn run(args: CreateArgs) -> Result<(), BundlebaseError> {
    let config = load_config(args.config.as_deref())?;
    let state: Arc<dyn BundleFacade> = match BundleBuilder::create(&args.bundle, config).await {
        Ok(builder) => builder,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("A bundle already exists") {
                return Err(format!(
                    "A bundle already exists at '{}'. To modify an existing bundle, use 'bundlebase extend' instead.",
                    args.bundle
                ).into());
            }
            return Err(e);
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
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|e| {
                        BundlebaseError::from(format!("Failed to read SQL from stdin: {}", e))
                    })?;
                let trimmed = buf.trim().to_string();
                if trimmed.is_empty() { None } else { Some(trimmed) }
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
        let commit_sql = format!("COMMIT '{}'", message.replace('\'', "''"));
        bundlebase_cli::repl::execute_single(state, &commit_sql, OutputFormat::Table).await?;
    }

    Ok(())
}
