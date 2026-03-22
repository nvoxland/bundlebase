//! The `extend` subcommand — execute a single mutating command and exit.
//! Also used by the hidden `execute` alias.
//!
//! Opens the bundle in read-write mode (via `extend()`), allowing
//! ATTACH, COMMIT, FILTER, DROP, and all other mutating commands.
//! Auto-commits after execution if there are uncommitted changes.

use super::load_config;
use bundlebase::{Bundle, BundleFacade, BundlebaseError};
use bundlebase_cli::OutputFormat;
use clap::Args;
use std::io::Read;
use std::sync::Arc;

/// Execute one or more semicolon-separated SQL statements against a bundle in read-write mode
#[derive(Args, Debug)]
pub struct ExtendArgs {
    /// Path or URL to the source bundle
    #[arg(long)]
    pub bundle: String,

    /// Extend to a new directory instead of modifying in place
    #[arg(long)]
    pub to: Option<String>,

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

/// Generate a commit message from the SQL command.
fn auto_commit_message(sql: &str) -> String {
    // Normalize whitespace and trim
    let normalized: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");

    // Truncate if too long
    if normalized.len() <= 72 {
        normalized
    } else {
        format!("{}...", &normalized[..69])
    }
}

pub async fn run(args: ExtendArgs) -> Result<(), BundlebaseError> {
    let sql = match args.sql {
        Some(s) => s,
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| BundlebaseError::from(format!("Failed to read SQL from stdin: {}", e)))?;
            let trimmed = buf.trim().to_string();
            if trimmed.is_empty() {
                eprintln!("Error: No SQL provided. Pass SQL as an argument or pipe it via stdin.");
                std::process::exit(1);
            }
            trimmed
        }
    };

    let config = load_config(args.config.as_deref())?;
    let bundle = match Bundle::open(&args.bundle, config).await {
        Ok(b) => b,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("does not exist") || msg.contains("not found") || msg.contains("No such file") || msg.contains("init.yaml") {
                return Err(format!(
                    "No bundle found at '{}'. To create a new bundle, use 'bundlebase create'.\n\nUnderlying error: {}",
                    args.bundle, msg
                ).into());
            }
            return Err(e);
        }
    };
    let state: Arc<dyn BundleFacade> = bundle
        .extend(args.to.as_deref())
        .await?;

    // Execute the user's command
    bundlebase_cli::repl::execute_single(state.clone(), &sql, args.format).await?;

    // Auto-commit if there are uncommitted changes
    if !state.status_changes().is_empty() {
        let message = args.message.unwrap_or_else(|| auto_commit_message(&sql));
        let commit_sql = format!("COMMIT '{}'", message.replace('\'', "''"));
        bundlebase_cli::repl::execute_single(state, &commit_sql, OutputFormat::Table).await?;
    }

    Ok(())
}
