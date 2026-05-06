//! The `server` subcommand — Arrow Flight SQL server.

use super::{load_config, BundleArgs};
use bundlebase::BundleBuilder;
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

    /// Create a new bundle at the path before starting the server.
    /// Errors if a bundle already exists at that path.
    #[arg(long, default_value = "false")]
    pub create: bool,
}

pub async fn run(args: ServerArgs) -> Result<(), BundlebaseError> {
    info!(
        "Opening bundle at: {}{}",
        args.bundle.bundle,
        if args.bundle.read_only {
            " (read-only)"
        } else {
            ""
        }
    );

    let config = load_config(args.bundle.config.as_deref())?;

    if args.create {
        if args.bundle.read_only {
            return Err(BundlebaseError::from(
                "--create cannot be combined with --read-only".to_string(),
            ));
        }
        let builder = match BundleBuilder::create(&args.bundle.bundle, config.clone()).await {
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
        // Commit an initial empty state so the Flight service can open the
        // bundle on the first request — `BundleBuilder::create` only reserves
        // the location; the on-disk bundle isn't established until commit.
        builder.commit("Initial commit").await?;
    }

    let port = args.port.unwrap_or(50051);
    let addr = format!("{}:{}", args.host, port)
        .parse()
        .map_err(|e| BundlebaseError::from(format!("Invalid address: {}", e)))?;

    bundlebase_cli::flight::start(&args.bundle.bundle, config, args.bundle.read_only, addr).await
}
