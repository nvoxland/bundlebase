//! Shared low-level utilities used by connectors and source orchestration.
//!
//! Contains pattern matching, argument parsing, URL helpers, streaming primitives,
//! and Parquet conversion.

use crate::BundlebaseError;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::StreamExt;
use glob::Pattern;
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

// ---------------------------------------------------------------------------
// Pattern helpers
// ---------------------------------------------------------------------------

/// Parse glob patterns from a comma-separated string.
pub fn parse_patterns(patterns_str: &str) -> Result<Vec<Pattern>, BundlebaseError> {
    patterns_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|p| {
            Pattern::new(p).map_err(|e| {
                BundlebaseError::from(format!("Invalid glob pattern '{}': {}", p, e))
            })
        })
        .collect()
}

/// Get patterns from args, returning compiled patterns.
pub fn get_patterns(args: &HashMap<String, String>) -> Result<Vec<Pattern>, BundlebaseError> {
    let patterns_str = args
        .get("patterns")
        .map(|s| s.as_str())
        .unwrap_or("**/*");
    parse_patterns(patterns_str)
}

/// Check if a URL matches any of the compiled patterns.
pub fn matches_patterns(url: &Url, patterns: &[Pattern]) -> bool {
    let path = url.path();
    let filename = path.rsplit('/').next().unwrap_or(path);
    patterns
        .iter()
        .any(|p| p.matches(filename) || p.matches(path.trim_start_matches('/')))
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

/// Check if should_copy is enabled from args (default: true).
pub fn should_copy(args: &HashMap<String, String>) -> bool {
    args.get("copy").map(|s| s != "false").unwrap_or(true)
}

/// Validate the "copy" argument if present.
pub fn validate_copy_arg(
    function_name: &str,
    args: &HashMap<String, String>,
) -> Result<(), BundlebaseError> {
    if let Some(copy_val) = args.get("copy") {
        if copy_val != "true" && copy_val != "false" {
            return Err(format!(
                "Function '{}': 'copy' argument must be 'true' or 'false', got '{}'",
                function_name, copy_val
            )
            .into());
        }
    }
    Ok(())
}

/// Get a required argument from args, returning an error if missing.
pub fn require_arg<'a>(
    args: &'a HashMap<String, String>,
    name: &str,
    function_name: &str,
) -> Result<&'a str, BundlebaseError> {
    args.get(name).map(|s| s.as_str()).ok_or_else(|| {
        BundlebaseError::from(format!(
            "Function '{}' requires a '{}' argument",
            function_name, name
        ))
    })
}

/// Parse and validate a URL from args.
pub fn require_url(
    args: &HashMap<String, String>,
    function_name: &str,
) -> Result<Url, BundlebaseError> {
    let url_str = require_arg(args, "url", function_name)?;
    Url::parse(url_str).map_err(|e| {
        BundlebaseError::from(format!("Invalid URL '{}': {}", url_str, e))
    })
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

/// Extract a filename from a URL path.
pub fn filename_from_url(url: &Url) -> String {
    url.path_segments()
        .and_then(|s| s.last())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "data".to_string())
}

/// Information extracted from an HTTP HEAD response.
#[derive(Debug, Clone)]
pub struct HttpHeadInfo {
    /// Version string derived from ETag or Last-Modified header.
    pub version: String,
    /// Raw Content-Type header value, if present (e.g., "text/csv; charset=utf-8").
    pub content_type: Option<String>,
}

/// Build a descriptive error message for an HTTP error status code.
///
/// Includes the status code, reason phrase, an actionable hint, and optionally
/// a truncated snippet of the server response body.
pub fn http_status_error(
    url: &Url,
    status: reqwest::StatusCode,
    body: Option<&str>,
) -> String {
    let hint = match status.as_u16() {
        401 => " The server requires authentication. Check if credentials are needed.",
        403 => " Access is forbidden. Check if the URL requires authorization or an API key.",
        404 => " The URL was not found. Verify the URL is correct and the resource exists.",
        429 => " Too many requests. Try again later or reduce request frequency.",
        500 => " The server encountered an internal error. The service may be temporarily unavailable.",
        502 | 503 | 504 => " The service is temporarily unavailable. Try again later.",
        _ => " Verify the URL is correct and accessible.",
    };
    let body_snippet = match body {
        Some(b) if !b.trim().is_empty() => {
            let truncated = if b.len() > 200 { &b[..200] } else { b };
            format!(" Server response: {}", truncated.trim())
        }
        _ => String::new(),
    };
    format!(
        "HTTP {} error from '{}': {}.{}{}",
        status.as_u16(),
        url,
        status.canonical_reason().unwrap_or("Unknown error"),
        hint,
        body_snippet,
    )
}

/// Read version and content-type from an HTTP(S) URL via HEAD request.
///
/// Returns an error if the server responds with a non-success status code.
/// Redirects (3xx) are followed automatically by the HTTP client.
pub async fn read_http_head_info(url: &Url) -> Result<HttpHeadInfo, BundlebaseError> {
    let response = reqwest::Client::new()
        .head(url.as_str())
        .send()
        .await
        .map_err(|e| BundlebaseError::from(format!("Failed to HEAD '{}': {}", url, e)))?;

    if !response.status().is_success() {
        return Err(http_status_error(url, response.status(), None).into());
    }

    let version = if let Some(etag) = response.headers().get("etag") {
        etag.to_str().unwrap_or("unknown").to_string()
    } else if let Some(lm) = response.headers().get("last-modified") {
        lm.to_str().unwrap_or("unknown").to_string()
    } else {
        format!("status-{}", response.status().as_u16())
    };

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    Ok(HttpHeadInfo {
        version,
        content_type,
    })
}

/// Detect data format by inspecting the first bytes of content.
///
/// Returns `"parquet"`, `"json"`, or `"csv"`. Returns `None` for empty content.
pub fn detect_format_from_bytes(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 4 && &data[0..4] == b"PAR1" {
        return Some("parquet");
    }
    // Skip BOM if present
    let skip = if data.len() >= 3 && &data[0..3] == &[0xEF, 0xBB, 0xBF] {
        3
    } else {
        0
    };
    let first_non_ws = data[skip..].iter().find(|b| !b.is_ascii_whitespace());
    match first_non_ws {
        Some(b'{') | Some(b'[') => Some("json"),
        Some(_) => Some("csv"),
        None => None,
    }
}

// ---------------------------------------------------------------------------
// Streaming helpers
// ---------------------------------------------------------------------------

/// A stream wrapper that holds a guard value alive until the stream is dropped.
pub struct GuardedStream<S> {
    inner: S,
    _guard: Arc<tempfile::NamedTempFile>,
}

impl<S: futures::Stream + Unpin> futures::Stream for GuardedStream<S> {
    type Item = S::Item;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.inner).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// Stream bytes from a temp file in chunks using `ReaderStream`.
pub fn stream_from_temp_file(
    temp: tempfile::NamedTempFile,
) -> BoxStream<'static, Result<Bytes, std::io::Error>> {
    let std_file = match temp.reopen() {
        Ok(f) => f,
        Err(e) => {
            return Box::pin(futures::stream::once(async move { Err(e) }));
        }
    };
    let async_file = tokio::fs::File::from_std(std_file);
    let reader_stream = tokio_util::io::ReaderStream::new(async_file);

    let _guard = Arc::new(temp);
    Box::pin(GuardedStream {
        inner: reader_stream,
        _guard,
    })
}

// ---------------------------------------------------------------------------
// Parquet conversion
// ---------------------------------------------------------------------------

/// Convert a stream of Arrow RecordBatches to Parquet bytes.
pub async fn record_batch_stream_to_parquet(
    mut batch_stream: BoxStream<'static, Result<arrow::record_batch::RecordBatch, BundlebaseError>>,
) -> Result<Bytes, BundlebaseError> {
    let first_batch = batch_stream
        .next()
        .await
        .ok_or("Arrow batch stream was empty")??;

    let schema = first_batch.schema();
    let mut buffer = Vec::new();
    {
        let props = parquet::file::properties::WriterProperties::builder().build();
        let mut writer =
            parquet::arrow::ArrowWriter::try_new(&mut buffer, schema, Some(props))
                .map_err(|e| format!("Failed to create parquet writer: {}", e))?;

        writer
            .write(&first_batch)
            .map_err(|e| format!("Failed to write batch to parquet: {}", e))?;

        while let Some(batch_result) = batch_stream.next().await {
            let batch = batch_result?;
            writer
                .write(&batch)
                .map_err(|e| format!("Failed to write batch to parquet: {}", e))?;
        }

        writer
            .close()
            .map_err(|e| format!("Failed to close parquet writer: {}", e))?;
    }

    Ok(Bytes::from(buffer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_patterns_single() {
        let patterns = parse_patterns("*.parquet").expect("failed to parse");
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("file.parquet"));
        assert!(!patterns[0].matches("file.csv"));
    }

    #[test]
    fn test_parse_patterns_multiple() {
        let patterns = parse_patterns("*.parquet, *.csv").expect("failed to parse");
        assert_eq!(patterns.len(), 2);
        assert!(patterns[0].matches("file.parquet"));
        assert!(patterns[1].matches("file.csv"));
    }

    #[test]
    fn test_parse_patterns_invalid() {
        let result = parse_patterns("[invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_patterns_default() {
        let args = HashMap::new();
        let patterns = get_patterns(&args).expect("failed to get patterns");
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("anything"));
    }

    #[test]
    fn test_require_arg_present() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "http://example.com".to_string());
        assert!(require_arg(&args, "url", "test").is_ok());
    }

    #[test]
    fn test_require_arg_missing() {
        let args = HashMap::new();
        assert!(require_arg(&args, "url", "test").is_err());
    }

    #[test]
    fn test_filename_from_url() {
        let url = Url::parse("http://example.com/path/to/file.csv").expect("valid url");
        assert_eq!(filename_from_url(&url), "file.csv");
    }

    #[test]
    fn test_detect_format_parquet_magic() {
        let data = b"PAR1\x00\x00\x00\x00some parquet data";
        assert_eq!(detect_format_from_bytes(data), Some("parquet"));
    }

    #[test]
    fn test_detect_format_json_object() {
        assert_eq!(detect_format_from_bytes(b"  {\"key\": \"value\"}"), Some("json"));
    }

    #[test]
    fn test_detect_format_json_array() {
        assert_eq!(detect_format_from_bytes(b"[1,2,3]"), Some("json"));
    }

    #[test]
    fn test_detect_format_csv() {
        assert_eq!(detect_format_from_bytes(b"col1,col2\na,b"), Some("csv"));
    }

    #[test]
    fn test_detect_format_csv_with_bom() {
        let mut data = vec![0xEF, 0xBB, 0xBF];
        data.extend_from_slice(b"col1,col2\na,b");
        assert_eq!(detect_format_from_bytes(&data), Some("csv"));
    }

    #[test]
    fn test_detect_format_json_with_bom() {
        let mut data = vec![0xEF, 0xBB, 0xBF];
        data.extend_from_slice(b"{\"key\": 1}");
        assert_eq!(detect_format_from_bytes(&data), Some("json"));
    }

    #[test]
    fn test_detect_format_empty() {
        assert_eq!(detect_format_from_bytes(b""), None);
    }

    #[test]
    fn test_detect_format_whitespace_only() {
        assert_eq!(detect_format_from_bytes(b"   \n\t  "), None);
    }

    #[test]
    fn test_validate_copy_arg_valid() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "true".to_string());
        assert!(validate_copy_arg("test", &args).is_ok());
    }

    #[test]
    fn test_validate_copy_arg_invalid() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "maybe".to_string());
        assert!(validate_copy_arg("test", &args).is_err());
    }
}
