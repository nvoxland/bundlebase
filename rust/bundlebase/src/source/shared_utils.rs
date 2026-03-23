//! Shared low-level utilities used by connectors and `source_utils`.
//!
//! Contains pattern matching, argument parsing, URL helpers, streaming primitives,
//! and Parquet conversion — everything that is needed by multiple modules in the
//! source subsystem.

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
///
/// Returns compiled patterns ready for matching.
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
///
/// Uses the "patterns" arg if present, otherwise defaults to "**/*".
pub fn get_patterns(args: &HashMap<String, String>) -> Result<Vec<Pattern>, BundlebaseError> {
    let patterns_str = args
        .get("patterns")
        .map(|s| s.as_str())
        .unwrap_or("**/*");
    parse_patterns(patterns_str)
}

/// Check if a URL matches any of the compiled patterns.
///
/// Matches against both the filename and the full path portion of the URL.
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
///
/// Returns the last path segment, or "data" if none found.
pub fn filename_from_url(url: &Url) -> String {
    url.path_segments()
        .and_then(|s| s.last())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "data".to_string())
}

/// Read version from an HTTP(S) URL using ETag or Last-Modified header.
///
/// Sends a HEAD request and extracts version information from headers.
/// Uses ETag if available, otherwise Last-Modified, otherwise falls back to status code.
pub async fn read_http_version(url: &Url) -> Result<String, BundlebaseError> {
    let response = reqwest::Client::new()
        .head(url.as_str())
        .send()
        .await
        .map_err(|e| BundlebaseError::from(format!("Failed to HEAD '{}': {}", url, e)))?;

    // Use ETag if available, otherwise Last-Modified, otherwise status
    if let Some(etag) = response.headers().get("etag") {
        return Ok(etag.to_str().unwrap_or("unknown").to_string());
    }
    if let Some(lm) = response.headers().get("last-modified") {
        return Ok(lm.to_str().unwrap_or("unknown").to_string());
    }
    Ok(format!("status-{}", response.status().as_u16()))
}

// ---------------------------------------------------------------------------
// Streaming helpers
// ---------------------------------------------------------------------------

/// A stream wrapper that holds a guard value alive until the stream is dropped.
///
/// Used to prevent temp file cleanup while the stream is being consumed.
pub(crate) struct GuardedStream<S> {
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
///
/// The `NamedTempFile` handle is held via `Arc` so the underlying file
/// is not deleted until the stream is fully consumed or dropped.
pub(crate) fn stream_from_temp_file(
    temp: tempfile::NamedTempFile,
) -> BoxStream<'static, Result<Bytes, std::io::Error>> {
    // Reopen to get a separate file descriptor for async reading
    let std_file = match temp.reopen() {
        Ok(f) => f,
        Err(e) => {
            return Box::pin(futures::stream::once(async move { Err(e) }));
        }
    };
    let async_file = tokio::fs::File::from_std(std_file);
    let reader_stream = tokio_util::io::ReaderStream::new(async_file);

    // Hold `temp` alive via Arc so the file isn't deleted while streaming
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
///
/// Reads batches one at a time from the stream, writes each to an ArrowWriter,
/// and returns the resulting Parquet file as `Bytes`. This keeps peak memory to
/// one batch plus the Parquet output buffer.
pub async fn record_batch_stream_to_parquet(
    mut batch_stream: futures::stream::BoxStream<'static, Result<arrow::record_batch::RecordBatch, BundlebaseError>>,
) -> Result<Bytes, BundlebaseError> {
    // Read the first batch to get the schema
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
    fn test_parse_patterns_empty_parts() {
        let patterns = parse_patterns("*.parquet,,*.csv").expect("failed to parse");
        assert_eq!(patterns.len(), 2);
    }

    #[test]
    fn test_get_patterns_default() {
        let args = HashMap::new();
        let patterns = get_patterns(&args).expect("failed to get patterns");
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("anything"));
    }

    #[test]
    fn test_get_patterns_custom() {
        let mut args = HashMap::new();
        args.insert("patterns".to_string(), "*.csv".to_string());
        let patterns = get_patterns(&args).expect("failed to get patterns");
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].matches("file.csv"));
        assert!(!patterns[0].matches("file.parquet"));
    }

    #[test]
    fn test_matches_patterns_filename() {
        let patterns = parse_patterns("*.parquet").expect("failed to parse");
        let url = Url::parse("https://example.com/data/file.parquet").expect("valid url");
        assert!(matches_patterns(&url, &patterns));
    }

    #[test]
    fn test_matches_patterns_path() {
        let patterns = parse_patterns("data/*.parquet").expect("failed to parse");
        let url = Url::parse("https://example.com/data/file.parquet").expect("valid url");
        assert!(matches_patterns(&url, &patterns));
    }

    #[test]
    fn test_matches_patterns_no_match() {
        let patterns = parse_patterns("*.csv").expect("failed to parse");
        let url = Url::parse("https://example.com/data/file.parquet").expect("valid url");
        assert!(!matches_patterns(&url, &patterns));
    }

    #[test]
    fn test_should_copy_default() {
        let args = HashMap::new();
        assert!(should_copy(&args));
    }

    #[test]
    fn test_should_copy_true() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "true".to_string());
        assert!(should_copy(&args));
    }

    #[test]
    fn test_should_copy_false() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "false".to_string());
        assert!(!should_copy(&args));
    }

    #[test]
    fn test_validate_copy_arg_valid() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "true".to_string());
        assert!(validate_copy_arg("test", &args).is_ok());

        args.insert("copy".to_string(), "false".to_string());
        assert!(validate_copy_arg("test", &args).is_ok());
    }

    #[test]
    fn test_validate_copy_arg_invalid() {
        let mut args = HashMap::new();
        args.insert("copy".to_string(), "invalid".to_string());
        let result = validate_copy_arg("test", &args);
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("'copy' argument must be"));
    }

    #[test]
    fn test_require_arg_present() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com".to_string());
        let result = require_arg(&args, "url", "test");
        assert_eq!(result.expect("should be ok"), "https://example.com");
    }

    #[test]
    fn test_require_arg_missing() {
        let args = HashMap::new();
        let result = require_arg(&args, "url", "test");
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("requires a 'url' argument"));
    }

    #[test]
    fn test_require_url_valid() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "https://example.com/data/".to_string());
        let result = require_url(&args, "test");
        assert!(result.is_ok());
        assert_eq!(
            result.expect("should be ok").as_str(),
            "https://example.com/data/"
        );
    }

    #[test]
    fn test_require_url_invalid() {
        let mut args = HashMap::new();
        args.insert("url".to_string(), "not-a-url".to_string());
        let result = require_url(&args, "test");
        assert!(result.is_err());
        assert!(result
            .err()
            .expect("expected error")
            .to_string()
            .contains("Invalid URL"));
    }

    #[test]
    fn test_filename_from_url() {
        let url = Url::parse("https://example.com/data/file.parquet").expect("valid url");
        assert_eq!(filename_from_url(&url), "file.parquet");
    }

    #[test]
    fn test_filename_from_url_no_filename() {
        let url = Url::parse("https://example.com/").expect("valid url");
        assert_eq!(filename_from_url(&url), "data");
    }

    #[test]
    fn test_filename_from_url_nested() {
        let url = Url::parse("s3://bucket/path/to/data.csv").expect("valid url");
        assert_eq!(filename_from_url(&url), "data.csv");
    }
}
