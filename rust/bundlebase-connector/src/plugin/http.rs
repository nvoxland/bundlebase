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
///   Auto-detected from the URL extension if omitted.
pub struct HttpConnector;

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
                    description: "Force file format (csv, json, parquet). Auto-detected from URL if omitted",
                    required: false,
                    default: None,
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

        // Determine format from explicit arg or URL extension
        let format = if let Some(fmt) = args.get("format") {
            fmt.to_lowercase()
        } else {
            shared_utils::filename_from_url(&url)
                .rsplit('.')
                .next()
                .unwrap_or("csv")
                .to_lowercase()
        };

        // Read version from HTTP headers (ETag/Last-Modified)
        let version = shared_utils::read_http_version(&url)
            .await
            .unwrap_or_else(|_| "unknown".to_string());

        Ok(vec![DiscoveredLocation {
            location: url.to_string(),
            must_copy: false,
            format,
            version,
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
        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "https://example.com/data/lake_quality.csv".to_string(),
        );
        let config = crate::test_utils::test_config();

        let locations = connector
            .discover(&args, &HashSet::new(), &config)
            .await
            .unwrap();

        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].location, "https://example.com/data/lake_quality.csv");
        assert_eq!(locations[0].format, "csv");
    }

    #[tokio::test]
    async fn test_discover_explicit_format() {
        let connector = HttpConnector;
        let mut args = HashMap::new();
        args.insert(
            "url".to_string(),
            "https://api.example.com/download?type=data".to_string(),
        );
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
