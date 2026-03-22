//! The `extend` subcommand — execute a single mutating command and exit.
//! Also used by the hidden `execute` alias.
//!
//! Opens the bundle in read-write mode (via `extend()`), allowing
//! ATTACH, COMMIT, FILTER, DROP, and all other mutating commands.
//! Auto-commits after execution if there are uncommitted changes.

use super::{open_bundle, BundleArgs};
use bundlebase::BundlebaseError;
use bundlebase_cli::OutputFormat;
use clap::Args;
use std::io::Read;

/// Execute one or more semicolon-separated SQL statements against a bundle in read-write mode
#[derive(Args, Debug)]
pub struct ExtendArgs {
    #[command(flatten)]
    pub bundle: BundleArgs,

    /// SQL or bundlebase command(s) to execute, semicolon-separated (reads from stdin if omitted)
    pub sql: Option<String>,

    /// Output format for query results
    #[arg(long, value_enum, default_value = "table")]
    pub format: OutputFormat,

    /// Commit message (auto-generated from the command if omitted)
    #[arg(long, short = 'm')]
    pub message: Option<String>,
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

    let state = open_bundle(&args.bundle).await?;

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
