//! The `generate-report` subcommand — generate a PDF report from markdown with data blocks.

use bundlebase::Bundle;
use bundlebase::BundleFacade;
use bundlebase_common::BundlebaseError;
use clap::Args;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// Generate a PDF report from markdown with embedded data queries and charts
#[derive(Args, Debug)]
pub struct ReportArgs {
    /// Path to markdown file with report template
    pub input: String,

    /// Output PDF path
    #[arg(long, short)]
    pub output: String,

    /// Disable 'Created by Bundlebase' footer
    #[arg(long)]
    pub no_branding: bool,
}

pub async fn run(args: ReportArgs) -> Result<(), BundlebaseError> {
    let markdown = tokio::fs::read_to_string(&args.input).await.map_err(|e| {
        BundlebaseError::from(format!("Failed to read input file '{}': {}", args.input, e))
    })?;

    info!("Generating report from '{}'", args.input);

    let resolver = CliBundleResolver::new();

    let msg = bundlebase_report::generate_report(
        &markdown,
        &resolver,
        &args.output,
        !args.no_branding,
    )
    .await?;

    println!("{}", msg);
    Ok(())
}

/// Bundle resolver for CLI context — opens bundles by path/URL, caching across blocks.
struct CliBundleResolver {
    cache: Mutex<HashMap<String, Arc<dyn BundleFacade>>>,
}

impl CliBundleResolver {
    fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl bundlebase_report::BundleResolver for CliBundleResolver {
    async fn resolve(
        &self,
        bundle_ref: &str,
    ) -> Result<Arc<dyn BundleFacade>, BundlebaseError> {
        // Check cache first
        {
            let cache = self.cache.lock().await;
            if let Some(bundle) = cache.get(bundle_ref) {
                return Ok(bundle.clone());
            }
        }

        // Open bundle in read-only mode
        info!("Opening bundle: {}", bundle_ref);
        let bundle = Bundle::open(bundle_ref, None).await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to open bundle '{}': {}",
                bundle_ref, e
            ))
        })?;

        // Cache for reuse
        let arc_bundle: Arc<dyn BundleFacade> = bundle;
        self.cache
            .lock()
            .await
            .insert(bundle_ref.to_string(), arc_bundle.clone());

        Ok(arc_bundle)
    }
}
