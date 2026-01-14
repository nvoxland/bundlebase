//! Built-in "web_scrape" source function.
//!
//! Fetches a webpage, extracts links from `<a href="...">` elements,
//! and downloads files that match specified glob patterns.

use super::source_function::{
    ArgSpec, AttachedFileInfo, DiscoveredLocation, FetchAction, SourceFunction, SyncMode,
};
use super::source_utils;
use crate::io::IOReadWriteDir;
use crate::{BundleConfig, BundlebaseError};
use async_trait::async_trait;
use scraper::{Html, Selector};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Built-in "web_scrape" source function.
///
/// Fetches a webpage and downloads all linked files matching the specified patterns.
///
/// Arguments:
/// - `url` (required): The webpage URL to fetch and parse for links
/// - `patterns` (optional): Comma-separated glob patterns to match href attributes
///   (e.g., "*.parquet,*.csv"). Defaults to "**/*" (all links)
/// - `copy` (optional): "true" to copy files into bundle's data_dir (default),
///   "false" to reference files at their original URL
/// - `mode` (optional): Sync mode for fetch:
///   - "add" (default): Only attach new files
///   - "update": Add new files and replace changed files
///   - "sync": Add new, replace changed, and remove files no longer at source
pub struct WebScrapeFunction;

#[async_trait]
impl SourceFunction for WebScrapeFunction {
    fn name(&self) -> &str {
        "web_scrape"
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec {
                name: "url",
                description: "The webpage URL to fetch and parse for links",
                required: true,
                default: None,
            },
            ArgSpec {
                name: "patterns",
                description: "Comma-separated glob patterns to match href attributes",
                required: false,
                default: Some("**/*"),
            },
            ArgSpec {
                name: "copy",
                description: "Whether to copy files into bundle's data directory",
                required: false,
                default: Some("true"),
            },
            ArgSpec {
                name: "mode",
                description: "Sync mode: 'add' (default), 'update', or 'sync'",
                required: false,
                default: Some("add"),
            },
        ]
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // Use default validation for required args and copy validation
        self.default_validate_args(args)?;

        // Additional: URL must be HTTP or HTTPS
        let url = source_utils::require_url(args, self.name())?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(format!(
                "Function '{}': URL must be http:// or https://, got '{}'",
                self.name(),
                url.scheme()
            )
            .into());
        }

        // Validate mode if provided
        if let Some(mode) = args.get("mode") {
            SyncMode::from_arg(mode)?;
        }

        Ok(())
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        let base_url = source_utils::require_url(args, self.name())?;
        let patterns = source_utils::get_patterns(args)?;

        // Fetch the webpage
        let html = self.fetch_page(&base_url).await?;

        // Extract and resolve all links, filter by pattern and already-attached
        let locations = self
            .extract_links(&html, &base_url)
            .into_iter()
            .filter(|url| source_utils::matches_patterns(url, &patterns))
            .filter(|url| !attached_locations.contains(url.as_str()))
            .map(DiscoveredLocation::from_url)
            .collect();

        Ok(locations)
    }

    async fn fetch_with_mode(
        &self,
        args: &HashMap<String, String>,
        attached_files: &HashMap<String, AttachedFileInfo>,
        data_dir: &dyn IOReadWriteDir,
        config: Arc<BundleConfig>,
        mode: SyncMode,
    ) -> Result<Vec<FetchAction>, BundlebaseError> {
        let base_url = source_utils::require_url(args, self.name())?;
        let patterns = source_utils::get_patterns(args)?;

        // Fetch and extract links (same as discover)
        let html = self.fetch_page(&base_url).await?;
        let discovered: Vec<DiscoveredLocation> = self
            .extract_links(&html, &base_url)
            .into_iter()
            .filter(|url| source_utils::matches_patterns(url, &patterns))
            .map(DiscoveredLocation::from_url)
            .collect();

        // Use shared sync logic
        let should_copy = source_utils::should_copy(args);
        let config = config.clone();

        source_utils::process_sync_mode(
            discovered,
            attached_files,
            data_dir,
            mode,
            |url| Box::pin(source_utils::read_http_version(url)),
            |loc| {
                let config = config.clone();
                async move {
                    source_utils::materialize_url(&loc.url, should_copy, data_dir, &config).await
                }
            },
        )
        .await
    }

    // Uses default materialize() and fetch() implementations
}

impl WebScrapeFunction {
    /// Fetch the HTML content of a webpage.
    async fn fetch_page(&self, url: &Url) -> Result<String, BundlebaseError> {
        let response = reqwest::get(url.as_str())
            .await
            .map_err(|e| BundlebaseError::from(format!("Failed to fetch '{}': {}", url, e)))?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to fetch '{}': HTTP {}",
                url,
                response.status()
            )
            .into());
        }

        response
            .text()
            .await
            .map_err(|e| {
                BundlebaseError::from(format!("Failed to read response from '{}': {}", url, e))
            })
    }

    /// Extract all links from HTML and resolve them to absolute URLs.
    fn extract_links(&self, html: &str, base_url: &Url) -> Vec<Url> {
        let document = Html::parse_document(html);
        let selector = Selector::parse("a[href]").expect("valid selector");

        document
            .select(&selector)
            .filter_map(|element| {
                let href = element.value().attr("href")?;
                self.resolve_url(href, base_url)
            })
            .collect()
    }

    /// Resolve a potentially relative URL against a base URL.
    fn resolve_url(&self, href: &str, base_url: &Url) -> Option<Url> {
        let href = href.trim();

        // Skip empty, javascript:, mailto:, data:, and fragment-only URLs
        if href.is_empty()
            || href.starts_with("javascript:")
            || href.starts_with("mailto:")
            || href.starts_with("data:")
            || href.starts_with('#')
        {
            return None;
        }

        // Try to parse as absolute URL first
        if let Ok(url) = Url::parse(href) {
            // Only accept http/https URLs
            if url.scheme() == "http" || url.scheme() == "https" {
                return Some(url);
            }
            return None;
        }

        // Resolve relative URL against base
        base_url.join(href).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        let func = WebScrapeFunction;
        assert_eq!(func.name(), "web_scrape");
    }

    #[test]
    fn test_arg_specs() {
        let func = WebScrapeFunction;
        let specs = func.arg_specs();
        assert_eq!(specs.len(), 4);
        assert!(specs.iter().any(|s| s.name == "url" && s.required));
        assert!(specs.iter().any(|s| s.name == "patterns" && !s.required));
        assert!(specs.iter().any(|s| s.name == "copy" && !s.required));
        assert!(specs.iter().any(|s| s.name == "mode" && !s.required));
    }

    #[test]
    fn test_validate_args_with_url() {
        let func = WebScrapeFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com/data/".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_missing_url() {
        let func = WebScrapeFunction;
        let args = HashMap::new();

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires a 'url' argument"));
    }

    #[test]
    fn test_validate_args_invalid_url() {
        let func = WebScrapeFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "not-a-valid-url".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid URL"));
    }

    #[test]
    fn test_validate_args_non_http_url() {
        let func = WebScrapeFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "ftp://example.com/data/".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be http:// or https://"));
    }

    #[test]
    fn test_validate_args_copy_true() {
        let func = WebScrapeFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com/data/".to_string());
        args.insert("copy".to_string(), "true".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_copy_false() {
        let func = WebScrapeFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com/data/".to_string());
        args.insert("copy".to_string(), "false".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_copy_invalid() {
        let func = WebScrapeFunction;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com/data/".to_string());
        args.insert("copy".to_string(), "invalid".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("'copy' argument must be 'true' or 'false'"));
    }

    #[test]
    fn test_extract_links() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/data/").unwrap();
        let html = r#"
            <html>
            <body>
                <a href="file1.parquet">File 1</a>
                <a href="subdir/file2.csv">File 2</a>
                <a href="https://other.com/file3.json">File 3</a>
                <a href="/absolute/path.parquet">Absolute</a>
                <a href="../parent/file.parquet">Parent</a>
            </body>
            </html>
        "#;

        let links = func.extract_links(html, &base_url);
        let urls: Vec<String> = links.iter().map(|u| u.to_string()).collect();

        assert!(urls.contains(&"https://example.com/data/file1.parquet".to_string()));
        assert!(urls.contains(&"https://example.com/data/subdir/file2.csv".to_string()));
        assert!(urls.contains(&"https://other.com/file3.json".to_string()));
        assert!(urls.contains(&"https://example.com/absolute/path.parquet".to_string()));
        assert!(urls.contains(&"https://example.com/parent/file.parquet".to_string()));
        assert_eq!(links.len(), 5);
    }

    #[test]
    fn test_extract_links_skips_invalid() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/").unwrap();
        let html = r##"
            <html>
            <body>
                <a href="javascript:void(0)">JS Link</a>
                <a href="mailto:test@example.com">Email</a>
                <a href="data:text/plain,hello">Data URL</a>
                <a href="#section">Fragment</a>
                <a href="">Empty</a>
                <a href="   ">Whitespace</a>
                <a href="file.txt">Valid</a>
            </body>
            </html>
        "##;

        let links = func.extract_links(html, &base_url);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].as_str(), "https://example.com/file.txt");
    }

    #[test]
    fn test_resolve_url_absolute() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/data/").unwrap();

        let resolved = func.resolve_url("https://other.com/file.txt", &base_url);
        assert_eq!(resolved.unwrap().as_str(), "https://other.com/file.txt");
    }

    #[test]
    fn test_resolve_url_relative() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/data/").unwrap();

        let resolved = func.resolve_url("file.txt", &base_url);
        assert_eq!(
            resolved.unwrap().as_str(),
            "https://example.com/data/file.txt"
        );
    }

    #[test]
    fn test_resolve_url_parent() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/data/subdir/").unwrap();

        let resolved = func.resolve_url("../file.txt", &base_url);
        assert_eq!(
            resolved.unwrap().as_str(),
            "https://example.com/data/file.txt"
        );
    }

    #[test]
    fn test_resolve_url_absolute_path() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/data/").unwrap();

        let resolved = func.resolve_url("/other/file.txt", &base_url);
        assert_eq!(
            resolved.unwrap().as_str(),
            "https://example.com/other/file.txt"
        );
    }

    #[test]
    fn test_resolve_url_skips_javascript() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/").unwrap();

        assert!(func.resolve_url("javascript:void(0)", &base_url).is_none());
    }

    #[test]
    fn test_resolve_url_skips_mailto() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/").unwrap();

        assert!(func
            .resolve_url("mailto:test@example.com", &base_url)
            .is_none());
    }

    #[test]
    fn test_resolve_url_skips_data() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/").unwrap();

        assert!(func
            .resolve_url("data:text/plain,hello", &base_url)
            .is_none());
    }

    #[test]
    fn test_resolve_url_skips_fragment() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/").unwrap();

        assert!(func.resolve_url("#section", &base_url).is_none());
    }

    #[test]
    fn test_resolve_url_skips_empty() {
        let func = WebScrapeFunction;
        let base_url = Url::parse("https://example.com/").unwrap();

        assert!(func.resolve_url("", &base_url).is_none());
        assert!(func.resolve_url("   ", &base_url).is_none());
    }
}
