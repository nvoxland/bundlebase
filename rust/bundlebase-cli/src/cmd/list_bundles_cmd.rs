//! The `list-bundles` subcommand — discover bundles in a directory.
//!
//! Scans a directory (local or remote) for subdirectories that contain a bundle
//! and prints their name and description.
//!
//! # Examples
//!
//! ```bash
//! # List bundles in the current directory
//! bundlebase list-bundles
//!
//! # List bundles in a specific directory
//! bundlebase list-bundles /data/bundles
//!
//! # List bundles in an S3 bucket
//! bundlebase list-bundles s3://my-bucket/bundles/
//! ```

use bundlebase::{Bundle, BundleConfig, BundleFacade, INIT_FILENAME, META_DIR};
use bundlebase_common::BundlebaseError;
use clap::Args;
use std::sync::Arc;

/// List all bundles found in a directory
#[derive(Args, Debug)]
pub struct ListBundlesArgs {
    /// Path or URL to search for bundles (defaults to current directory)
    #[arg(long, default_value = ".")]
    pub path: String,
}

/// Discover bundle URLs under a directory by scanning for init manifest files.
///
/// Returns a sorted list of bundle root URLs found under `path`.
async fn find_bundle_urls(path: &str) -> Result<Vec<String>, BundlebaseError> {
    let config = Arc::new(BundleConfig::new(None)?);
    let dir = bundlebase_io::readable_dir_from_str(path, config).await?;

    let init_suffix = format!("{}/{}", META_DIR, INIT_FILENAME);

    let files = dir.list_files().await?;
    let mut bundle_urls: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let url_str = f.url.to_string();
            if url_str.ends_with(&init_suffix) {
                // Strip the /_bundlebase/00000000000000000.yaml suffix to get the bundle URL
                let bundle_url = url_str[..url_str.len() - init_suffix.len()].trim_end_matches('/');
                Some(bundle_url.to_string())
            } else {
                None
            }
        })
        .collect();

    bundle_urls.sort();
    Ok(bundle_urls)
}

pub async fn run(args: ListBundlesArgs) -> Result<(), BundlebaseError> {
    let bundle_urls = find_bundle_urls(&args.path).await?;

    if bundle_urls.is_empty() {
        println!("(no bundles)");
        return Ok(());
    }

    for bundle_url in &bundle_urls {
        // Extract the directory name from the URL for display
        let display_name = bundle_url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(bundle_url);

        match Bundle::open(bundle_url, None).await {
            Ok(bundle) => {
                match bundle.name() {
                    Some(name) => println!("{} : {}", display_name, name),
                    None => println!("{} : (name not set)", display_name),
                }
                match bundle.description() {
                    Some(description) => {
                        for line in description.lines() {
                            println!("    {}", line);
                        }
                    }
                    None => println!("    (description not set)"),
                }
                println!();
            }
            Err(e) => {
                eprintln!("Warning: Failed to open bundle at '{}': {}", bundle_url, e);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bundlebase::{BundleBuilder, BundleFacade};
    use bundlebase_cli::OutputFormat;

    fn init() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            bundlebase_catalog::init();
        });
    }

    /// Generate a unique memory URL prefix to isolate tests from each other.
    fn unique_prefix(label: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("System time before UNIX epoch")
            .as_nanos();
        format!("memory:///list_bundles_{}_{}", label, nanos)
    }

    #[tokio::test]
    async fn test_empty_directory_finds_no_bundles() {
        init();
        let prefix = unique_prefix("empty");
        let urls = find_bundle_urls(&prefix)
            .await
            .expect("find_bundle_urls failed");
        assert!(urls.is_empty(), "Expected no bundles, got: {:?}", urls);
    }

    #[tokio::test]
    async fn test_finds_single_bundle() {
        init();
        let prefix = unique_prefix("single");
        let bundle_url = format!("{}/my_bundle", prefix);

        let builder = BundleBuilder::create(&bundle_url, None)
            .await
            .expect("Failed to create bundle");
        builder.commit("Initial").await.expect("Failed to commit");

        let urls = find_bundle_urls(&prefix)
            .await
            .expect("find_bundle_urls failed");
        assert_eq!(urls.len(), 1, "Expected 1 bundle, got: {:?}", urls);
        assert!(
            urls[0].ends_with("/my_bundle"),
            "Unexpected URL: {}",
            urls[0]
        );
    }

    #[tokio::test]
    async fn test_finds_multiple_bundles_sorted() {
        init();
        let prefix = unique_prefix("multi");

        // Create bundles in non-alphabetical order
        for name in ["charlie", "alpha", "bravo"] {
            let url = format!("{}/{}", prefix, name);
            let builder = BundleBuilder::create(&url, None)
                .await
                .expect("Failed to create bundle");
            builder.commit("Initial").await.expect("Failed to commit");
        }

        let urls = find_bundle_urls(&prefix)
            .await
            .expect("find_bundle_urls failed");
        assert_eq!(urls.len(), 3, "Expected 3 bundles, got: {:?}", urls);

        // Verify sorted order
        let names: Vec<&str> = urls
            .iter()
            .map(|u| u.rsplit('/').next().expect("no slash"))
            .collect();
        assert_eq!(names, vec!["alpha", "bravo", "charlie"]);
    }

    #[tokio::test]
    async fn test_run_shows_name_and_description() {
        init();
        let prefix = unique_prefix("meta");
        let bundle_url = format!("{}/named_bundle", prefix);

        let builder = BundleBuilder::create(&bundle_url, None)
            .await
            .expect("Failed to create bundle");

        let facade: Arc<dyn BundleFacade> = builder;
        bundlebase_cli::repl::execute_single(
            facade.clone(),
            "SET NAME 'My Bundle'; SET DESCRIPTION 'A test bundle'; COMMIT 'Set metadata'",
            OutputFormat::Table,
        )
        .await
        .expect("Failed to set metadata");

        // Verify the bundle can be opened and has the right metadata
        let bundle = Bundle::open(&bundle_url, None)
            .await
            .expect("Failed to open");
        assert_eq!(bundle.name(), Some("My Bundle".to_string()));
        assert_eq!(bundle.description(), Some("A test bundle".to_string()));

        // Verify it's discoverable
        let urls = find_bundle_urls(&prefix)
            .await
            .expect("find_bundle_urls failed");
        assert_eq!(urls.len(), 1);
    }

    #[tokio::test]
    async fn test_run_succeeds_on_empty_directory() {
        init();
        let prefix = unique_prefix("run_empty");
        let result = run(ListBundlesArgs { path: prefix }).await;
        assert!(result.is_ok(), "Expected success, got: {:?}", result.err());
    }
}
