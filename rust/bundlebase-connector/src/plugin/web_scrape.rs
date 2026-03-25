//! Built-in "web_scrape" connector.
//!
//! Fetches a webpage, extracts links from `<a href="...">` elements,
//! and downloads files that match specified glob patterns.

use bundlebase_common::connector::{
    ArgSpec, DiscoveredLocation, SourceData, Connector, ConnectorSignature,
};
use bundlebase_common::source_utils as shared_utils;
use bundlebase_common::{ConfigProvider, BundlebaseError};
use async_trait::async_trait;
use scraper::{Html, Selector};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Built-in "web_scrape" connector.
///
/// Fetches a webpage and downloads all linked files matching the specified patterns.
///
/// Arguments:
/// - `url` (required): The webpage URL to fetch and parse for links
/// - `patterns` (optional): Comma-separated glob patterns to match href attributes
///   (e.g., "*.parquet,*.csv"). Defaults to "**/*" (all links)
/// - `copy` (optional): "true" to copy files into bundle's data_dir (default),
///   "false" to reference files at their original URL
pub struct WebScrapeConnector;

#[async_trait]
impl Connector for WebScrapeConnector {
    fn signature(&self) -> ConnectorSignature {
        ConnectorSignature {
            name: "web_scrape".to_string(),
            arg_specs: vec![
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
            ],
            accepts_extra_args: false,
        }
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        // URL must be HTTP or HTTPS
        let url = shared_utils::require_url(args, "web_scrape")?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(format!(
                "Function 'web_scrape': URL must be http:// or https://, got '{}'",
                url.scheme()
            )
            .into());
        }
        Ok(())
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        _attached_locations: &HashSet<String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        let base_url = shared_utils::require_url(args, "web_scrape")?;
        let patterns = shared_utils::get_patterns(args)?;

        // Fetch the webpage
        let html = self.fetch_page(&base_url).await?;

        // Extract and resolve all links, filter by pattern
        let mut locations = Vec::new();
        for url in self.extract_links(&html, &base_url) {
            if !shared_utils::matches_patterns(&url, &patterns) {
                continue;
            }

            // Get format from URL extension
            let format = shared_utils::filename_from_url(&url)
                .rsplit('.')
                .next()
                .unwrap_or("dat")
                .to_string();

            // Read version from HTTP headers
            let version = shared_utils::read_http_version(&url)
                .await
                .unwrap_or_else(|_| "unknown".to_string());

            locations.push(DiscoveredLocation {
                location: url.to_string(),
                must_copy: false,
                format,
                version,
            });
        }

        Ok(locations)
    }

    async fn data(
        &self,
        _location: &DiscoveredLocation,
        _args: &HashMap<String, String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<SourceData>, BundlebaseError> {
        // Web scrape uses stable_url for downloading
        Ok(None)
    }

    async fn stable_url(
        &self,
        location: &DiscoveredLocation,
        _args: &HashMap<String, String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<Url>, BundlebaseError> {
        let url = Url::parse(&location.location).map_err(|e| {
            BundlebaseError::from(format!(
                "Invalid URL in discovered location '{}': {}",
                location.location, e
            ))
        })?;

        Ok(Some(url))
    }
}

impl WebScrapeConnector {
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

        response.text().await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read response from '{}': {}",
                url, e
            ))
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
    fn test_signature() {
        let func = WebScrapeConnector;
        let sig = func.signature();
        assert_eq!(sig.name, "web_scrape");
        assert_eq!(sig.arg_specs.len(), 3);
        assert!(sig.arg_specs.iter().any(|s| s.name == "url" && s.required));
        assert!(sig
            .arg_specs
            .iter()
            .any(|s| s.name == "patterns" && !s.required));
        assert!(sig
            .arg_specs
            .iter()
            .any(|s| s.name == "copy" && !s.required));
    }

    #[test]
    fn test_validate_args_with_url() {
        let func = WebScrapeConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "https://example.com/data/".to_string(),
        );
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_missing_url() {
        
        let func = WebScrapeConnector;
        let args = HashMap::new();

        let result = { let sig = func.signature(); bundlebase_common::connector::validate_connector_args(&args, &sig) };
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("requires a 'url' argument"));
    }

    #[test]
    fn test_validate_args_invalid_url() {
        let func = WebScrapeConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "not-a-valid-url".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("Invalid URL"));
    }

    #[test]
    fn test_validate_args_non_http_url() {
        let func = WebScrapeConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "ftp://example.com/data/".to_string());

        let result = func.validate_args(&args);
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("must be http:// or https://"));
    }

    #[test]
    fn test_validate_args_copy_true() {
        let func = WebScrapeConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "https://example.com/data/".to_string(),
        );
        args.insert("copy".to_string(), "true".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_copy_false() {
        let func = WebScrapeConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "https://example.com/data/".to_string(),
        );
        args.insert("copy".to_string(), "false".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_connector_args_copy_invalid() {
        
        let func = WebScrapeConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "https://example.com/data/".to_string(),
        );
        args.insert("copy".to_string(), "invalid".to_string());

        let result = { let sig = func.signature(); bundlebase_common::connector::validate_connector_args(&args, &sig) };
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("'copy' argument must be 'true' or 'false'"));
    }

    #[test]
    fn test_extract_links() {
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/data/").expect("valid url");
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
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/").expect("valid url");
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
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/data/").expect("valid url");

        let resolved = func.resolve_url("https://other.com/file.txt", &base_url);
        assert_eq!(
            resolved.expect("should resolve").as_str(),
            "https://other.com/file.txt"
        );
    }

    #[test]
    fn test_resolve_url_relative() {
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/data/").expect("valid url");

        let resolved = func.resolve_url("file.txt", &base_url);
        assert_eq!(
            resolved.expect("should resolve").as_str(),
            "https://example.com/data/file.txt"
        );
    }

    #[test]
    fn test_resolve_url_parent() {
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/data/subdir/").expect("valid url");

        let resolved = func.resolve_url("../file.txt", &base_url);
        assert_eq!(
            resolved.expect("should resolve").as_str(),
            "https://example.com/data/file.txt"
        );
    }

    #[test]
    fn test_resolve_url_absolute_path() {
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/data/").expect("valid url");

        let resolved = func.resolve_url("/other/file.txt", &base_url);
        assert_eq!(
            resolved.expect("should resolve").as_str(),
            "https://example.com/other/file.txt"
        );
    }

    #[test]
    fn test_resolve_url_skips_javascript() {
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/").expect("valid url");
        assert!(func.resolve_url("javascript:void(0)", &base_url).is_none());
    }

    #[test]
    fn test_resolve_url_skips_mailto() {
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/").expect("valid url");
        assert!(func
            .resolve_url("mailto:test@example.com", &base_url)
            .is_none());
    }

    #[test]
    fn test_resolve_url_skips_data() {
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/").expect("valid url");
        assert!(func
            .resolve_url("data:text/plain,hello", &base_url)
            .is_none());
    }

    #[test]
    fn test_resolve_url_skips_fragment() {
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/").expect("valid url");
        assert!(func.resolve_url("#section", &base_url).is_none());
    }

    #[test]
    fn test_resolve_url_skips_empty() {
        let func = WebScrapeConnector;
        let base_url = Url::parse("https://example.com/").expect("valid url");
        assert!(func.resolve_url("", &base_url).is_none());
        assert!(func.resolve_url("   ", &base_url).is_none());
    }
}
