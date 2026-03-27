//! Built-in "http" connector.
//!
//! Downloads data directly from an HTTP(S) URL. Unlike `web_scrape` which
//! parses HTML pages for links, this connector treats the URL as a direct
//! link to a data file (CSV, JSON, Parquet, etc.).

use bundlebase_common::connector::{ArgSpec, Connector, ConnectorSignature, DiscoveredLocation, SourceData};
use bundlebase_common::source_utils as shared_utils;
use bundlebase_common::{ConfigProvider, BundlebaseError};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Built-in "http" connector.
///
/// Downloads data from a single HTTP(S) URL. The URL should point directly
/// to a data file (CSV, JSON, Parquet).
///
/// Arguments:
/// - `url` (required): The HTTP(S) URL to download
/// - `format` (optional): Force the file format (csv, json, parquet).
///   Auto-detected from Content-Type header, URL extension, or content inspection if omitted.
pub struct HttpConnector;

/// Map a Content-Type header value to a data format string.
fn format_from_content_type(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next().unwrap_or("").trim().to_lowercase();
    match mime.as_str() {
        "text/csv" => Some("csv"),
        "application/json" | "application/x-ndjson" | "application/jsonl" => Some("json"),
        "application/vnd.apache.parquet" => Some("parquet"),
        "text/tab-separated-values" => Some("tsv"),
        _ => None,
    }
}

/// Extract format from URL file extension, if recognized.
fn format_from_url_extension(url: &Url) -> Option<String> {
    let filename = shared_utils::filename_from_url(url);
    let ext = filename.rsplit('.').next()?.to_lowercase();
    let known = ["csv", "json", "jsonl", "parquet", "tsv", "xml"];
    if known.contains(&ext.as_str()) {
        Some(ext)
    } else {
        None
    }
}

#[async_trait]
impl Connector for HttpConnector {
    fn signature(&self) -> ConnectorSignature {
        ConnectorSignature {
            name: "http".to_string(),
            arg_specs: vec![
                ArgSpec {
                    name: "url",
                    description: "The HTTP(S) URL to download data from",
                    required: true,
                    default: None,
                },
                ArgSpec {
                    name: "format",
                    description: "File format (csv, json, parquet, auto). Default: auto (detect from Content-Type, URL extension, or content inspection)",
                    required: false,
                    default: Some("auto"),
                },
            ],
            accepts_extra_args: false,
        }
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        let url = shared_utils::require_url(args, "http")?;
        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(format!(
                "Connector 'http': URL must be http:// or https://, got '{}'",
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
        let url = shared_utils::require_url(args, "http")?;

        // Read HEAD info (version + content-type) in one request.
        // Propagate server errors (e.g. 500, 503) so we fail early rather than
        // silently loading error content as data.
        let head_info = shared_utils::read_http_head_info(&url).await?;

        // Format detection priority:
        // 1. Explicit format arg (unless "auto")
        // 2. Content-Type header
        // 3. URL file extension
        // 4. "auto" — will be resolved by content inspection after download
        let explicit = args.get("format").map(|f| f.to_lowercase());
        let format = if let Some(ref fmt) = explicit {
            if fmt != "auto" {
                fmt.clone()
            } else {
                "auto".to_string()
            }
        } else if let Some(fmt) = head_info.content_type.as_deref().and_then(format_from_content_type) {
            fmt.to_string()
        } else if let Some(fmt) = format_from_url_extension(&url) {
            fmt
        } else {
            "auto".to_string()
        };

        Ok(vec![DiscoveredLocation {
            location: url.to_string(),
            must_copy: false,
            format,
            version: head_info.version,
        }])
    }

    async fn data(
        &self,
        _location: &DiscoveredLocation,
        _args: &HashMap<String, String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<SourceData>, BundlebaseError> {
        // Uses stable_url for downloading — the URL itself is the data location
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature() {
        let connector = HttpConnector;
        let sig = connector.signature();
        assert_eq!(sig.name, "http");
        assert_eq!(sig.arg_specs.len(), 2);
        assert!(sig.arg_specs.iter().any(|s| s.name == "url" && s.required));
        assert!(sig
            .arg_specs
            .iter()
            .any(|s| s.name == "format" && !s.required));
    }

    #[test]
    fn test_format_from_content_type_csv() {
        assert_eq!(format_from_content_type("text/csv"), Some("csv"));
    }

    #[test]
    fn test_format_from_content_type_csv_with_charset() {
        assert_eq!(format_from_content_type("text/csv; charset=utf-8"), Some("csv"));
    }

    #[test]
    fn test_format_from_content_type_json() {
        assert_eq!(format_from_content_type("application/json"), Some("json"));
    }

    #[test]
    fn test_format_from_content_type_ndjson() {
        assert_eq!(format_from_content_type("application/x-ndjson"), Some("json"));
    }

    #[test]
    fn test_format_from_content_type_parquet() {
        assert_eq!(format_from_content_type("application/vnd.apache.parquet"), Some("parquet"));
    }

    #[test]
    fn test_format_from_content_type_tsv() {
        assert_eq!(format_from_content_type("text/tab-separated-values"), Some("tsv"));
    }

    #[test]
    fn test_format_from_content_type_octet_stream() {
        assert_eq!(format_from_content_type("application/octet-stream"), None);
    }

    #[test]
    fn test_format_from_content_type_html() {
        assert_eq!(format_from_content_type("text/html"), None);
    }

    #[test]
    fn test_format_from_content_type_empty() {
        assert_eq!(format_from_content_type(""), None);
    }

    #[test]
    fn test_format_from_url_extension_csv() {
        let url = Url::parse("https://example.com/data.csv").unwrap();
        assert_eq!(format_from_url_extension(&url), Some("csv".to_string()));
    }

    #[test]
    fn test_format_from_url_extension_parquet() {
        let url = Url::parse("https://example.com/data.parquet").unwrap();
        assert_eq!(format_from_url_extension(&url), Some("parquet".to_string()));
    }

    #[test]
    fn test_format_from_url_extension_none_for_api_url() {
        let url = Url::parse("https://api.example.com/download?type=data").unwrap();
        assert_eq!(format_from_url_extension(&url), None);
    }

    #[test]
    fn test_format_from_url_extension_none_for_unknown() {
        let url = Url::parse("https://example.com/file.xyz").unwrap();
        assert_eq!(format_from_url_extension(&url), None);
    }

    #[test]
    fn test_validate_args_valid_url() {
        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com/data.csv".to_string());
        assert!(connector.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_http_url() {
        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "http://example.com/data.csv".to_string());
        assert!(connector.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_missing_url() {
        
        let connector = HttpConnector;
        let args = HashMap::new();
        let result = { let sig = connector.signature(); bundlebase_common::connector::validate_connector_args(&args, &sig) };
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("requires a 'url' argument"));
    }

    #[test]
    fn test_validate_args_non_http_url() {
        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), "ftp://example.com/data.csv".to_string());
        let result = connector.validate_args(&args);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("must be http:// or https://"));
    }

    #[tokio::test]
    async fn test_discover_csv_from_url_extension() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/data/lake_quality.csv", server.uri()));
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].format, "csv");
    }

    #[tokio::test]
    async fn test_discover_explicit_format() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/download?type=data", server.uri()));
        args.insert("format".to_string(), "json".to_string());
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].format, "json");
    }

    #[tokio::test]
    async fn test_discover_explicit_auto_skips_content_type_and_extension() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/csv"),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/data.csv", server.uri()));
        args.insert("format".to_string(), "auto".to_string());
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        // Explicit auto bypasses content-type and extension detection
        assert_eq!(locations[0].format, "auto");
    }

    #[tokio::test]
    async fn test_discover_format_from_content_type() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json"),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/data", server.uri()));
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        assert_eq!(locations[0].format, "json");
    }

    #[tokio::test]
    async fn test_discover_explicit_format_overrides_content_type() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/csv"),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/data", server.uri()));
        args.insert("format".to_string(), "parquet".to_string());
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        assert_eq!(locations[0].format, "parquet");
    }

    #[tokio::test]
    async fn test_discover_content_type_over_url_extension() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json"),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/data.csv", server.uri()));
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        // Content-Type wins over URL extension
        assert_eq!(locations[0].format, "json");
    }

    #[tokio::test]
    async fn test_discover_url_extension_when_octet_stream() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "application/octet-stream"),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/data.parquet", server.uri()));
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        // Falls through to URL extension
        assert_eq!(locations[0].format, "parquet");
    }

    #[tokio::test]
    async fn test_discover_auto_when_nothing_matches() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "application/octet-stream"),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/data", server.uri()));
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        assert_eq!(locations[0].format, "auto");
    }

    #[tokio::test]
    async fn test_discover_no_content_type_falls_to_extension() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(wiremock::ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/data.json", server.uri()));
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        assert_eq!(locations[0].format, "json");
    }

    #[tokio::test]
    async fn test_discover_500_returns_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/data.csv", server.uri()));
        let config = crate::test_utils::test_config();

        let result = connector.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("500"), "Error should mention status code: {}", err);
        assert!(err.contains("Internal Server Error"), "Error should include reason: {}", err);
    }

    #[tokio::test]
    async fn test_discover_503_returns_descriptive_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(wiremock::ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/data", server.uri()));
        let config = crate::test_utils::test_config();

        let result = connector.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("503"), "Error should mention status code: {}", err);
        assert!(err.contains("unavailable"), "Error should suggest service unavailable: {}", err);
    }

    #[tokio::test]
    async fn test_discover_404_returns_descriptive_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/missing.csv", server.uri()));
        let config = crate::test_utils::test_config();

        let result = connector.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("404"), "Error should mention status code: {}", err);
        assert!(err.contains("not found"), "Error should mention not found: {}", err);
    }

    #[tokio::test]
    async fn test_discover_401_returns_descriptive_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(wiremock::ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/private/data.csv", server.uri()));
        let config = crate::test_utils::test_config();

        let result = connector.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("401"), "Error should mention status code: {}", err);
        assert!(err.contains("authentication"), "Error should mention authentication: {}", err);
    }

    #[tokio::test]
    async fn test_discover_follows_redirect() {
        let server = wiremock::MockServer::start().await;

        // Redirect from /old to /new
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .and(wiremock::matchers::path("/old"))
            .respond_with(
                wiremock::ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/new", server.uri())),
            )
            .mount(&server)
            .await;

        // Final destination returns 200 with content-type
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .and(wiremock::matchers::path("/new"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/csv"),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/old", server.uri()));
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        assert_eq!(locations[0].format, "csv");
    }

    #[tokio::test]
    async fn test_stable_url() {
        let connector = HttpConnector;
        let location = DiscoveredLocation {
            location: "https://example.com/data.csv".to_string(),
            must_copy: false,
            format: "csv".to_string(),
            version: "v1".to_string(),
        };
        let config = crate::test_utils::test_config();

        let url = connector
            .stable_url(&location, &HashMap::new(), &config)
            .await
            .unwrap();

        assert_eq!(url.unwrap().as_str(), "https://example.com/data.csv");
    }
}
