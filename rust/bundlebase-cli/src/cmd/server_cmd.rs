//! The `server` subcommand — Arrow Flight SQL server.

use super::{load_config, BundleArgs};
use bundlebase_common::BundlebaseError;
use clap::Args;
use tracing::info;

/// Start Arrow Flight SQL server
#[derive(Args, Debug)]
pub struct ServerArgs {
    #[command(flatten)]
    pub bundle: BundleArgs,

    /// Host address to bind to
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// Port to listen on (default: 50051)
    #[arg(long)]
    pub port: Option<u16>,
}

pub async fn run(args: ServerArgs) -> Result<(), BundlebaseError> {
    info!(
        "Opening bundle at: {}{}",
        args.bundle.bundle,
        if args.bundle.read_only { " (read-only)" } else { "" }
    );

    let config = load_config(args.bundle.config.as_deref())?;
    let port = args.port.unwrap_or(50051);
    let addr = format!("{}:{}", args.host, port)
        .parse()
        .map_err(|e| BundlebaseError::from(format!("Invalid address: {}", e)))?;

    bundlebase_cli::flight::start(
        &args.bundle.bundle,
        config,
        args.bundle.read_only,
        addr,
    )
    .await
}
