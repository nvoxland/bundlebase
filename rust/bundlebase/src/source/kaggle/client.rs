//! HTTP client for the Kaggle REST API.

use super::{dataset_scope, API_KEY_CFG, URL_CFG, USERNAME_CFG};
use crate::{BundleConfig, BundlebaseError};

/// HTTP client for the Kaggle API that bundles credentials and base URL.
///
/// Every request goes through [`get`](KaggleClient::get) or
/// [`get_url`](KaggleClient::get_url), which apply Basic Auth automatically.
#[derive(Debug)]
pub(super) struct KaggleClient {
    client: reqwest::Client,
    pub(super) base_url: String,
    pub(super) username: String,
    pub(super) key: String,
}

impl KaggleClient {
    /// Build a `KaggleClient` from bundle config values.
    pub(super) fn from_config(config: &BundleConfig, dataset: &str) -> Result<Self, BundlebaseError> {
        let scope = dataset_scope(dataset);

        let base_url = config.get(&scope, &URL_CFG).ok_or_else(|| {
            BundlebaseError::from("Kaggle URL not configured")
        })?;
        let username = config.get(&scope, &USERNAME_CFG).ok_or_else(|| {
            BundlebaseError::from(
                "Kaggle username not found. Set via bundlebase config '/kaggle username'  or ~/.kaggle/kaggle.json",
            )
        })?;
        let key = config.get(&scope, &API_KEY_CFG).ok_or_else(|| {
            BundlebaseError::from(
                "Kaggle API key not found. Set via bundlebase config '/kaggle key', or ~/.kaggle/kaggle.json",
            )
        })?;

        Self::new(&base_url, &username, &key)
    }

    /// Explicit constructor (primarily for tests).
    pub(super) fn new(base_url: &str, username: &str, key: &str) -> Result<Self, BundlebaseError> {
        use reqwest::header;

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static("bundlebase"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| {
                BundlebaseError::from(format!("Failed to create Kaggle HTTP client: {}", e))
            })?;

        Ok(Self {
            client,
            base_url: base_url.to_string(),
            username: username.to_string(),
            key: key.to_string(),
        })
    }

    /// Start a GET request for a relative API path (e.g. `/api/v1/datasets/list`).
    pub(super) fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{}", self.base_url, path))
            .basic_auth(&self.username, Some(&self.key))
    }

    /// Start a GET request for an absolute URL (used when the full URL comes
    /// from a `DiscoveredLocation`).
    pub(super) fn get_url(&self, url: &str) -> reqwest::RequestBuilder {
        self.client
            .get(url)
            .basic_auth(&self.username, Some(&self.key))
    }
}
