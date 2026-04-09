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
    /// Path to the bundle
    #[arg(long)]
    pub bundle: String,

    /// Path to markdown file with report template
    #[arg(long)]
    pub input: Option<String>,

    /// ID of a stored report in the bundle
    #[arg(long)]
    pub id: Option<String>,

    /// Output PDF path
    #[arg(long, short)]
    pub output: String,

    /// Disable 'Created by Bundlebase' footer
    #[arg(long)]
    pub no_branding: bool,
}

pub async fn run(args: ReportArgs) -> Result<(), BundlebaseError> {
    // Validate exactly one of --input or --id
    match (&args.input, &args.id) {
        (Some(_), Some(_)) => {
            return Err(BundlebaseError::from(
                "Cannot specify both --input and --id. Use one or the other.",
            ));
        }
        (None, None) => {
            return Err(BundlebaseError::from(
                "Must specify either --input (markdown file) or --id (stored report id).",
            ));
        }
        _ => {}
    }

    // Open the bundle
    let bundle = Bundle::open(&args.bundle, None).await.map_err(|e| {
        BundlebaseError::from(format!("Failed to open bundle '{}': {}", args.bundle, e))
    })?;

    // Get markdown content
    let markdown = if let Some(input_path) = &args.input {
        tokio::fs::read_to_string(input_path).await.map_err(|e| {
            BundlebaseError::from(format!("Failed to read input file '{}': {}", input_path, e))
        })?
    } else {
        let report_id = args.id.as_ref().expect("already validated");
        let report = bundle.report_by_id(report_id).ok_or_else(|| {
            let reports = bundle.reports();
            let available: Vec<&String> = reports.keys().collect();
            let list = if available.is_empty() {
                "none".to_string()
            } else {
                available.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            };
            BundlebaseError::from(format!(
                "Report '{}' not found in bundle. Available reports: {}",
                report_id, list
            ))
        })?;
        report.content
    };

    info!("Generating report, output: '{}'", args.output);

    let resolver = CliBundleResolver::new(bundle.clone());

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

/// Bundle resolver for CLI context — uses the opened bundle for "." and "bundle" references,
/// and opens other bundles by path/URL, caching across blocks.
struct CliBundleResolver {
    primary_bundle: Arc<dyn BundleFacade>,
    cache: Mutex<HashMap<String, Arc<dyn BundleFacade>>>,
}

impl CliBundleResolver {
    fn new(bundle: Arc<dyn BundleFacade>) -> Self {
        Self {
            primary_bundle: bundle,
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
        // "." and "bundle" refer to the primary bundle
        if bundle_ref == "." || bundle_ref == "bundle" {
            return Ok(self.primary_bundle.clone());
        }

        // Check if this ref matches the primary bundle's URL
        let primary_url = self.primary_bundle.url().to_string();
        if bundle_ref == primary_url {
            return Ok(self.primary_bundle.clone());
        }

        // Check cache
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
