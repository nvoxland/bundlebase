//! The `upgrade-bundle` subcommand — update a bundle's format version to the current version.

use bundlebase::BundleBuilder;
use bundlebase_common::BundlebaseError;
use clap::Args;

/// Upgrade a bundle's format version to match the current bundlebase version
#[derive(Args, Debug)]
pub struct UpgradeBundleArgs {
    /// Path or URL of the bundle to upgrade
    #[arg(long)]
    pub bundle: String,

    /// Path to a YAML or JSON config file
    #[arg(long)]
    pub config: Option<String>,
}

pub async fn run(args: UpgradeBundleArgs) -> Result<(), BundlebaseError> {
    let config = super::load_config(args.config.as_deref())?;
    BundleBuilder::upgrade_bundle(&args.bundle, config).await?;

    let version = bundlebase_common::format_version_string();
    println!("Bundle upgraded to version {}", version);

    Ok(())
}
