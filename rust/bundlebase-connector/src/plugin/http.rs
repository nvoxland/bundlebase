//! Built-in "http" connector.
//!
//! Downloads data directly from an HTTP(S) URL. Unlike `web_scrape` which
//! parses HTML pages for links, this connector treats the URL as a direct
//! link to a data file (CSV, JSON, Parquet, etc.).
//!
//! Supports GET, POST, and PUT methods. Custom request headers can be passed
//! via the `headers` argument (one `Name: Value` pair per line).

use bundlebase_common::connector::{ArgSpec, Connector, ConnectorSignature, SourceFormat, DiscoveredLocation, SourceData};
use bundlebase_common::source_utils::{self as shared_utils, http_status_error, stream_response};
use bundlebase_common::{ConfigProvider, BundlebaseError};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use url::Url;

/// Built-in "http" connector.
///
/// Downloads data from a single HTTP(S) URL. The URL should point directly
/// to a data file (CSV, JSON, Parquet, etc.).
///
/// Arguments:
/// - `url` (required): The HTTP(S) URL to download
/// - `format` (optional): Force the file format (csv, json, parquet).
///   Auto-detected from Content-Type header, URL extension, or content inspection if omitted.
/// - `method` (optional): HTTP method — GET, POST, or PUT. Default: GET.
/// - `body` (optional): Request body string (for POST/PUT).
/// - `headers` (optional): Additional request headers, one per line as `Name: Value`.
/// - `head_supported` (optional): Whether the server supports HEAD requests. Default: true.
pub struct HttpConnector;

/// Parse the `method` arg. Returns uppercase method string, defaulting to "GET".
fn parse_method(args: &HashMap<String, String>) -> Result<String, BundlebaseError> {
    let method = args.get("method").map(|s| s.to_uppercase()).unwrap_or_else(|| "GET".to_string());
    match method.as_str() {
        "GET" | "POST" | "PUT" => Ok(method),
        other => Err(format!(
            "Connector 'http': unsupported method '{}'. Use GET, POST, or PUT.",
            other
        ).into()),
    }
}

/// Parse the `headers` arg into a Vec of (name, value) pairs.
///
/// Format: one `Name: Value` pair per line. Lines that don't contain `:` are ignored.
fn parse_headers(args: &HashMap<String, String>) -> Vec<(String, String)> {
    let Some(raw) = args.get("headers") else { return Vec::new() };
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            let colon = line.find(':')?;
            let name = line[..colon].trim().to_string();
            let value = line[colon + 1..].trim().to_string();
            if name.is_empty() { None } else { Some((name, value)) }
        })
        .collect()
}

/// Returns true if the connector needs to perform the request itself
/// (POST/PUT, or any custom headers, or a body is present).
fn needs_custom_request(args: &HashMap<String, String>) -> bool {
    let method = args.get("method").map(|s| s.to_uppercase()).unwrap_or_else(|| "GET".to_string());
    method != "GET" || args.contains_key("body") || args.contains_key("headers")
}

/// Perform the HTTP request using the configured method, body, and headers.
async fn perform_request(url: &Url, args: &HashMap<String, String>) -> Result<Bytes, BundlebaseError> {
    let method = parse_method(args)?;
    let headers = parse_headers(args);
    let body = args.get("body").cloned().unwrap_or_default();

    let client = reqwest::Client::new();
    let req_method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|e| BundlebaseError::from(format!("Invalid HTTP method '{}': {}", method, e)))?;

    let mut request = client.request(req_method, url.as_str());
    for (name, value) in &headers {
        request = request.header(name.as_str(), value.as_str());
    }
    if !body.is_empty() {
        request = request.body(body);
    }

    let response = request.send().await
        .map_err(|e| BundlebaseError::from(format!("HTTP request to '{}' failed: {}", url, e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().await.ok();
        return Err(http_status_error(url, status, body_text.as_deref()).into());
    }

    stream_response(&format!("Downloading {}", url), response).await
}

/// Map a Content-Type header value to a data format string.
fn format_from_content_type(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next().unwrap_or("").trim().to_lowercase();
    match mime.as_str() {
        "text/csv" => Some("csv"),
        "application/json" => Some("json"),
        "application/x-ndjson" | "application/jsonl" => Some("jsonl"),
        "application/vnd.apache.parquet" => Some("parquet"),
        "text/tab-separated-values" => Some("tsv"),
        _ => None,
    }
}

/// Extract format from URL file extension, if recognized.
fn format_from_url_extension(url: &Url) -> Option<String> {
    let filename = shared_utils::filename_from_url(url);
    let ext = filename.rsplit('.').next()?.to_lowercase();
    let known = ["csv", "json", "jsonl", "parquet", "tsv", "xml", "xlsx", "xls", "ods"];
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
                    name: "method",
                    description: "HTTP method: GET, POST, or PUT. Default: GET",
                    required: false,
                    default: Some("GET"),
                },
                ArgSpec {
                    name: "body",
                    description: "Request body string (for POST/PUT). Default: empty",
                    required: false,
                    default: None,
                },
                ArgSpec {
                    name: "headers",
                    description: "Additional request headers, one per line as 'Name: Value'. Example: 'Accept: text/csv\\nAuthorization: Bearer token'",
                    required: false,
                    default: None,
                },
                ArgSpec {
                    name: "format",
                    description: "File format (csv, json, parquet, auto). Default: auto (detect from Content-Type, URL extension, or content inspection)",
                    required: false,
                    default: Some("auto"),
                },
                ArgSpec {
                    name: "head_supported",
                    description: "Whether the server supports HEAD requests (true/false). Set to false for servers that fail on HEAD. Default: true",
                    required: false,
                    default: Some("true"),
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
        parse_method(args)?;
        Ok(())
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        _attached_locations: &HashSet<String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        let url = shared_utils::require_url(args, "http")?;

        // POST/PUT endpoints can't be meaningfully HEAD-probed, and the response
        // content-type depends on the body. Skip HEAD for non-GET requests.
        let method = parse_method(args)?;
        let is_get = method == "GET";

        // Check if HEAD is supported (default: true). HEAD is only used for GET.
        let head_supported = is_get && args.get("head_supported")
            .map(|v| v.to_lowercase() != "false")
            .unwrap_or(true);

        // Read HEAD info (version + content-type) unless HEAD is disabled or method is non-GET.
        let head_info = if head_supported {
            shared_utils::read_http_head_info(&url).await?
        } else {
            shared_utils::HttpHeadInfo {
                version: "unknown".to_string(),
                content_type: None,
            }
        };

        // Format detection priority:
        // 1. Explicit format arg (unless "auto")
        // 2. Content-Type header
        // 3. URL file extension
        // 4. "auto" — will be resolved by content inspection after download
        let explicit = args.get("format").map(|f| f.to_lowercase());
        let format_str = if let Some(ref fmt) = explicit {
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
        let format = SourceFormat::from_extension(&format_str);

        Ok(vec![DiscoveredLocation {
            location: url.to_string(),
            must_copy: false,
            format,
            version: head_info.version,
        }])
    }

    async fn data(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<SourceData>, BundlebaseError> {
        if !needs_custom_request(args) {
            // Simple GET with no custom headers — delegate to stable_url() path
            return Ok(None);
        }
        let url = Url::parse(&location.location).map_err(|e| {
            BundlebaseError::from(format!(
                "Invalid URL in discovered location '{}': {}",
                location.location, e
            ))
        })?;
        let bytes = perform_request(&url, args).await?;
        Ok(Some(SourceData::RawBytes(Box::pin(stream::once(async move {
            Ok::<Bytes, std::io::Error>(bytes)
        })))))
    }

    async fn stable_url(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<Url>, BundlebaseError> {
        if needs_custom_request(args) {
            // data() handles this request
            return Ok(None);
        }
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
        assert_eq!(sig.arg_specs.len(), 6);
        assert!(sig.arg_specs.iter().any(|s| s.name == "url" && s.required));
        assert!(sig.arg_specs.iter().any(|s| s.name == "method" && !s.required));
        assert!(sig.arg_specs.iter().any(|s| s.name == "body" && !s.required));
        assert!(sig.arg_specs.iter().any(|s| s.name == "headers" && !s.required));
        assert!(sig.arg_specs.iter().any(|s| s.name == "format" && !s.required));
        assert!(sig.arg_specs.iter().any(|s| s.name == "head_supported" && !s.required));
    }

    #[test]
    fn test_parse_method_default() {
        assert_eq!(parse_method(&HashMap::new()).unwrap(), "GET");
    }

    #[test]
    fn test_parse_method_post() {
        let mut args = HashMap::new();
        args.insert("method".to_string(), "POST".to_string());
        assert_eq!(parse_method(&args).unwrap(), "POST");
    }

    #[test]
    fn test_parse_method_put_lowercase() {
        let mut args = HashMap::new();
        args.insert("method".to_string(), "put".to_string());
        assert_eq!(parse_method(&args).unwrap(), "PUT");
    }

    #[test]
    fn test_parse_method_invalid() {
        let mut args = HashMap::new();
        args.insert("method".to_string(), "DELETE".to_string());
        assert!(parse_method(&args).is_err());
    }

    #[test]
    fn test_parse_headers_empty() {
        assert!(parse_headers(&HashMap::new()).is_empty());
    }

    #[test]
    fn test_parse_headers_single() {
        let mut args = HashMap::new();
        args.insert("headers".to_string(), "Accept: text/csv".to_string());
        let headers = parse_headers(&args);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0], ("Accept".to_string(), "text/csv".to_string()));
    }

    #[test]
    fn test_parse_headers_multiple_lines() {
        let mut args = HashMap::new();
        args.insert("headers".to_string(), "Accept: text/csv\nContent-Type: application/json".to_string());
        let headers = parse_headers(&args);
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0], ("Accept".to_string(), "text/csv".to_string()));
        assert_eq!(headers[1], ("Content-Type".to_string(), "application/json".to_string()));
    }

    #[test]
    fn test_parse_headers_value_with_colon() {
        let mut args = HashMap::new();
        args.insert("headers".to_string(), "Authorization: Bearer abc:def".to_string());
        let headers = parse_headers(&args);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0], ("Authorization".to_string(), "Bearer abc:def".to_string()));
    }

    #[test]
    fn test_needs_custom_request_default_get() {
        assert!(!needs_custom_request(&HashMap::new()));
    }

    #[test]
    fn test_needs_custom_request_post() {
        let mut args = HashMap::new();
        args.insert("method".to_string(), "POST".to_string());
        assert!(needs_custom_request(&args));
    }

    #[test]
    fn test_needs_custom_request_get_with_headers() {
        let mut args = HashMap::new();
        args.insert("headers".to_string(), "Accept: text/csv".to_string());
        assert!(needs_custom_request(&args));
    }

    #[test]
    fn test_needs_custom_request_get_with_body() {
        let mut args = HashMap::new();
        args.insert("body".to_string(), "some data".to_string());
        assert!(needs_custom_request(&args));
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
        assert_eq!(format_from_content_type("application/x-ndjson"), Some("jsonl"));
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
        assert_eq!(locations[0].format, SourceFormat::Csv);
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
        assert_eq!(locations[0].format, SourceFormat::Json);
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
        assert_eq!(locations[0].format, SourceFormat::Auto);
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

        assert_eq!(locations[0].format, SourceFormat::Json);
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

        assert_eq!(locations[0].format, SourceFormat::Parquet);
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
        assert_eq!(locations[0].format, SourceFormat::Json);
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
        assert_eq!(locations[0].format, SourceFormat::Parquet);
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

        assert_eq!(locations[0].format, SourceFormat::Auto);
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

        assert_eq!(locations[0].format, SourceFormat::Json);
    }

    #[tokio::test]
    async fn test_discover_500_returns_error_with_hint() {
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
        assert!(err.contains("head_supported"), "Error should suggest head_supported=false: {}", err);
    }

    #[tokio::test]
    async fn test_discover_500_succeeds_with_head_disabled() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(wiremock::ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/data.csv", server.uri()));
        args.insert("head_supported".to_string(), "false".to_string());
        let config = crate::test_utils::test_config();

        let locations = connector.discover(&args, &HashSet::new(), &config).await.unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].format, SourceFormat::Csv);
    }

    #[tokio::test]
    async fn test_discover_503_returns_error_with_hint() {
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
        assert!(err.contains("head_supported"), "Error should suggest head_supported=false: {}", err);
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
        assert!(!err.contains("head_supported"), "4xx errors should not suggest head_supported: {}", err);
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
        assert!(!err.contains("head_supported"), "4xx errors should not suggest head_supported: {}", err);
    }

    #[tokio::test]
    async fn test_discover_400_returns_descriptive_error() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(wiremock::ResponseTemplate::new(400))
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/search?bad=param", server.uri()));
        let config = crate::test_utils::test_config();

        let result = connector.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("400"), "Error should mention status code: {}", err);
        assert!(err.contains("rejected"), "Error should mention rejected: {}", err);
        assert!(!err.contains("head_supported"), "400 errors should not suggest head_supported: {}", err);
    }

    #[tokio::test]
    async fn test_discover_400_includes_warning_header() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("HEAD"))
            .respond_with(
                wiremock::ResponseTemplate::new(400)
                    .insert_header("warning", "299 WQP \"The value of organization=21MNPCA is not in the list of enumerated values.\""),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/search?organization=21MNPCA", server.uri()));
        let config = crate::test_utils::test_config();

        let result = connector.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(err.contains("400"), "Error should mention status code: {}", err);
        assert!(err.contains("enumerated values"), "Error should include warning header content: {}", err);
        assert!(!err.contains("head_supported"), "400 errors should not suggest head_supported: {}", err);
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

        assert_eq!(locations[0].format, SourceFormat::Csv);
    }

    #[tokio::test]
    async fn test_stable_url() {
        let connector = HttpConnector;
        let location = DiscoveredLocation {
            location: "https://example.com/data.csv".to_string(),
            must_copy: false,
            format: SourceFormat::Csv,
            version: "v1".to_string(),
        };
        let config = crate::test_utils::test_config();

        let url = connector
            .stable_url(&location, &HashMap::new(), &config)
            .await
            .unwrap();

        assert_eq!(url.unwrap().as_str(), "https://example.com/data.csv");
    }

    #[tokio::test]
    async fn test_stable_url_returns_none_for_post() {
        let connector = HttpConnector;
        let location = DiscoveredLocation {
            location: "https://example.com/api/query".to_string(),
            must_copy: false,
            format: SourceFormat::Csv,
            version: "unknown".to_string(),
        };
        let mut args = HashMap::new();
        args.insert("method".to_string(), "POST".to_string());
        let config = crate::test_utils::test_config();

        let url = connector.stable_url(&location, &args, &config).await.unwrap();
        assert!(url.is_none(), "POST requests should not use stable_url");
    }

    #[tokio::test]
    async fn test_stable_url_returns_none_when_headers_set() {
        let connector = HttpConnector;
        let location = DiscoveredLocation {
            location: "https://example.com/data.csv".to_string(),
            must_copy: false,
            format: SourceFormat::Csv,
            version: "v1".to_string(),
        };
        let mut args = HashMap::new();
        args.insert("headers".to_string(), "Authorization: Bearer token".to_string());
        let config = crate::test_utils::test_config();

        let url = connector.stable_url(&location, &args, &config).await.unwrap();
        assert!(url.is_none(), "Requests with custom headers should not use stable_url");
    }

    #[tokio::test]
    async fn test_discover_post_skips_head() {
        // Server only accepts POST, no HEAD endpoint
        let server = wiremock::MockServer::start().await;
        // No HEAD mock — would fail if HEAD were attempted

        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/query", server.uri()));
        args.insert("method".to_string(), "POST".to_string());
        args.insert("body".to_string(), "statecode=US%3A27".to_string());
        let config = crate::test_utils::test_config();

        // Should succeed without attempting HEAD
        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].version, "unknown");
    }

    #[tokio::test]
    async fn test_data_post_returns_bytes() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/query"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("col1,col2\na,b")
                    .insert_header("content-type", "text/csv"),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let location = DiscoveredLocation {
            location: format!("{}/api/query", server.uri()),
            must_copy: false,
            format: SourceFormat::Csv,
            version: "unknown".to_string(),
        };
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/query", server.uri()));
        args.insert("method".to_string(), "POST".to_string());
        args.insert("body".to_string(), "param=value".to_string());
        let config = crate::test_utils::test_config();

        let data = connector.data(&location, &args, &config).await.unwrap();
        assert!(data.is_some(), "POST request should return SourceData");
    }

    #[tokio::test]
    async fn test_data_get_with_custom_header() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::header("Accept", "text/csv"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("col1,col2\na,b"),
            )
            .mount(&server)
            .await;

        let connector = HttpConnector;
        let location = DiscoveredLocation {
            location: format!("{}/api/data", server.uri()),
            must_copy: false,
            format: SourceFormat::Csv,
            version: "unknown".to_string(),
        };
        let mut args = HashMap::new();
        args.insert("url".to_string(), format!("{}/api/data", server.uri()));
        args.insert("headers".to_string(), "Accept: text/csv".to_string());
        let config = crate::test_utils::test_config();

        let data = connector.data(&location, &args, &config).await.unwrap();
        assert!(data.is_some(), "GET with custom headers should return SourceData");
    }

    #[tokio::test]
    async fn test_data_returns_none_for_plain_get() {
        let connector = HttpConnector;
        let location = DiscoveredLocation {
            location: "https://example.com/data.csv".to_string(),
            must_copy: false,
            format: SourceFormat::Csv,
            version: "v1".to_string(),
        };
        let config = crate::test_utils::test_config();

        // Plain GET with no custom headers should return None (use stable_url path)
        let data = connector.data(&location, &HashMap::new(), &config).await.unwrap();
        assert!(data.is_none());
    }
}
