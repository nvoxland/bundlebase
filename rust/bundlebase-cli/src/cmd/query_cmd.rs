//! The `query` subcommand — execute a single SQL query and exit.
//! Also used by the hidden `execute` alias.
//!
//! Opens the bundle in read-only mode, so only SELECT, EXPLAIN,
//! and other non-mutating commands are allowed.

use super::load_config;
use bundlebase::{Bundle, BundlebaseError};
use bundlebase_cli::OutputFormat;
use clap::Args;
use std::io::Read;
use tracing::info;

/// Execute a single SQL query and exit
#[derive(Args, Debug)]
pub struct QueryArgs {
    /// Path or URL to the bundle
    #[arg(long)]
    pub bundle: String,

    /// SQL query to execute (reads from stdin if omitted)
    pub sql: Option<String>,

    /// Output format for query results
    #[arg(long, value_enum, default_value = "table")]
    pub format: OutputFormat,

    /// Path to a YAML or JSON config file
    #[arg(long)]
    pub config: Option<String>,
}

pub async fn run(args: QueryArgs) -> Result<(), BundlebaseError> {
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
    info!("Opening bundle in read-only mode: {}", args.bundle);
    let state = match Bundle::open(&args.bundle, config).await {
        Ok(s) => s,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("init.yaml") || msg.contains("not found") || msg.contains("No such file") || msg.contains("does not exist") {
                return Err(format!(
                    "No bundle found at '{}'. To create a new bundle, use 'bundlebase create'.\n\nUnderlying error: {}",
                    args.bundle, msg
                ).into());
            }
            return Err(e);
        }
    };

    bundlebase_cli::repl::execute_single(state, &sql, args.format).await
}
