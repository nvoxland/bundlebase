//! CLI subcommand modules and shared argument types.

pub mod extend_cmd;
pub mod mcp_cmd;
pub mod query_cmd;
pub mod repl_cmd;
pub mod server_cmd;
pub mod setup_agent_cmd;

use bundlebase::{Bundle, BundleBuilder, BundlebaseError, BundleFacade, PassedBundleConfig};
use clap::Args;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

/// Shared flags for opening or creating a bundle.
#[derive(Args, Debug, Clone)]
pub struct BundleArgs {
    /// Path or URL to the bundle
    #[arg(long)]
    pub bundle: String,

    /// Create a new bundle if it doesn't exist or is empty
    #[arg(long)]
    pub create: bool,

    /// Open bundle in read-only mode (default: false).
    /// When true, only SELECT and EXPLAIN commands are allowed.
    #[arg(long, default_value = "false")]
    pub read_only: bool,

    /// Path to a YAML or JSON config file
    #[arg(long)]
    pub config: Option<String>,
}

/// Open or create a bundle based on the shared flags.
pub async fn open_bundle(args: &BundleArgs) -> Result<Arc<dyn BundleFacade>, BundlebaseError> {
    if args.create && args.read_only {
        eprintln!("Error: Cannot use --create with --read-only=true. Creating a bundle requires write access.");
        eprintln!("Use --read-only=false with --create to create a new bundle.");
        std::process::exit(1);
    }

    let config = load_config(args.config.as_deref())?;

    let state: Arc<dyn BundleFacade> = if args.create {
        info!("Creating bundle at: {}", args.bundle);
        BundleBuilder::create(&args.bundle, config).await?
    } else if args.read_only {
        info!("Opening bundle in read-only mode: {}", args.bundle);
        Bundle::open(&args.bundle, config).await?
    } else {
        info!("Opening bundle in read-write mode: {}", args.bundle);
        Bundle::open(&args.bundle, config).await?.extend(None).await?
    };

    Ok(state)
}

/// Load a `PassedBundleConfig` from a YAML or JSON file, if a path is provided.
pub fn load_config(path: Option<&str>) -> Result<Option<PassedBundleConfig>, BundlebaseError> {
    let path = match path {
        Some(p) => p,
        None => return Ok(None),
    };

    let contents = std::fs::read_to_string(path)
        .map_err(|e| BundlebaseError::from(format!("Failed to read config file '{}': {}", path, e)))?;

    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let config: PassedBundleConfig = match ext {
        "json" => serde_json::from_str(&contents)
            .map_err(|e| BundlebaseError::from(format!("Failed to parse JSON config '{}': {}", path, e)))?,
        "yaml" | "yml" => serde_yaml_ng::from_str(&contents)
            .map_err(|e| BundlebaseError::from(format!("Failed to parse YAML config '{}': {}", path, e)))?,
        _ => {
            return Err(BundlebaseError::from(format!(
                "Unrecognized config file extension '{}'. Use .json, .yaml, or .yml",
                ext
            )))
        }
    };

    Ok(Some(config))
}
