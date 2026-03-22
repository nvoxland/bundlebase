//! The `server` subcommand — Arrow Flight SQL server.

use super::{load_config, BundleArgs};
use bundlebase::BundlebaseError;
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
    if args.bundle.create && args.bundle.read_only {
        eprintln!("Error: Cannot use --create with --read-only=true. Creating a bundle requires write access.");
        std::process::exit(1);
    }

    info!(
        "{} bundle at: {}{}",
        if args.bundle.create { "Creating" } else { "Opening" },
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
        args.bundle.create,
        args.bundle.read_only,
        addr,
    )
    .await
}
