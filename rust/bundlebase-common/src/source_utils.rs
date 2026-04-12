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
/// a truncated snippet of the server response body or Warning header.
pub fn http_status_error(
    url: &Url,
    status: reqwest::StatusCode,
    detail: Option<&str>,
) -> String {
    let hint = match status.as_u16() {
        400 => " The request was rejected by the server. Check URL parameters for invalid values.",
        401 => " The server requires authentication. Check if credentials are needed.",
        403 => " Access is forbidden. Check if the URL requires authorization or an API key.",
        404 => " The URL was not found. Verify the URL is correct and the resource exists.",
        429 => " Too many requests. Try again later or reduce request frequency.",
        500 => " The server encountered an internal error. The service may be temporarily unavailable.",
        502 | 503 | 504 => " The service is temporarily unavailable. Try again later.",
        _ => " Verify the URL is correct and accessible.",
    };
    let detail_snippet = match detail {
        Some(d) if !d.trim().is_empty() => {
            let truncated = if d.len() > 200 { &d[..200] } else { d };
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
        detail_snippet,
    )
}

/// Read version and content-type from an HTTP(S) URL via HEAD request.
///
/// Returns an error if the HEAD request fails or returns a non-success status.
/// Some servers don't support HEAD properly — use `head_supported=false` on the
/// http connector to skip this check entirely.
pub async fn read_http_head_info(url: &Url) -> Result<HttpHeadInfo, BundlebaseError> {
    let response = reqwest::Client::new()
        .head(url.as_str())
        .send()
        .await
        .map_err(|e| BundlebaseError::from(format!(
            "HEAD request failed for '{}': {}. \
             If this server doesn't support HEAD requests, retry with: \
             head_supported = 'false'",
            url, e
        )))?;

    if !response.status().is_success() {
        let status = response.status();
        let warning = response.headers().get("warning")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let base_err = http_status_error(url, status, warning.as_deref());
        return Err(base_err.into());
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

/// Stream an already-open `reqwest::Response` body into `Bytes`, reporting progress.
///
/// Opens a `ProgressScope` using `label` and `Content-Length` as the total (if known),
/// then streams response chunks and increments the scope for each one.
///
/// Use this when you already have a response object (e.g. after checking status/Content-Type).
/// Use [`download_url`] when you just have a URL.
pub async fn stream_response(
    label: &str,
    response: reqwest::Response,
) -> Result<Bytes, BundlebaseError> {
    use crate::progress::ProgressScope;

    let total = response.content_length();
    let progress = ProgressScope::new(label, total);
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| BundlebaseError::from(format!("Failed to read response: {}", e)))?;
        buffer.extend_from_slice(&chunk);
        progress.increment(chunk.len() as u64, None);
    }
    Ok(Bytes::from(buffer))
}

/// Download an HTTP(S) URL, reporting byte-level progress via the active `ProgressTracker`.
///
/// This is the canonical way to do a plain HTTP GET download in bundlebase. It:
/// - Opens a `ProgressScope` with the URL as the operation name and `Content-Length` as the total
/// - Streams response chunks, incrementing the scope for each chunk
/// - Validates the HTTP status code
///
/// Use this for simple GET downloads. For requests that need custom headers, method, or
/// content-type checking, make the request yourself and pass the response to
/// [`stream_response`].
pub async fn download_url(url: &Url) -> Result<Bytes, BundlebaseError> {
    let response = reqwest::get(url.as_str())
        .await
        .map_err(|e| BundlebaseError::from(format!("Failed to download '{}': {}", url, e)))?;

    if !response.status().is_success() {
        return Err(http_status_error(url, response.status(), None).into());
    }

    stream_response(&format!("Downloading {}", url), response).await
}

/// Detect data format by inspecting the first bytes of content.
///
/// Returns `"parquet"`, `"json"`, `"jsonl"`, or `"csv"`. Returns `None` for empty content.
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
        Some(b'[') => Some("json"),
        Some(b'{') => Some("jsonl"),
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
        let props = parquet::file::properties::WriterProperties::builder()
            .set_compression(parquet::basic::Compression::ZSTD(
                parquet::basic::ZstdLevel::try_new(3)
                    .unwrap_or(parquet::basic::ZstdLevel::default()),
            ))
            .set_max_row_group_size(128 * 1024) // 128K rows per row group
            .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Chunk)
            .set_bloom_filter_enabled(true)
            .build();
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

/// Convert JSON data to Parquet bytes using normalization options.
///
/// Handles wrapper objects (`json_record_path`), nested struct flattening (`json_sep`),
/// and broadcasting outer fields to every row (`json_meta`). Uses Arrow's type inference
/// so columns retain native types (Int64, Float64, Utf8, etc.).
///
/// # Arguments
/// * `data` - Raw JSON bytes
/// * `record_path` - Dot-notation path to the array of records (e.g. `"data"`, `"results.items"`).
///   Empty string means the root value is itself an array.
/// * `sep` - Separator for flattening nested field names (`"_"` → `user_name` from `user.name`)
/// * `meta_paths` - Outer-object field paths to broadcast as extra columns on every row
pub fn json_to_parquet_with_options(
    data: &[u8],
    record_path: &str,
    sep: &str,
    meta_paths: &[&str],
) -> Result<Bytes, BundlebaseError> {
    use std::io::Cursor;
    use std::sync::Arc;

    let root: serde_json::Value = serde_json::from_slice(data)
        .map_err(|e| BundlebaseError::from(format!("Invalid JSON: {}", e)))?;

    // Extract meta values from the outer object before navigating into record_path
    let meta_values: Vec<(String, serde_json::Value)> = meta_paths
        .iter()
        .filter_map(|path| {
            json_navigate_path(&root, path).map(|v| {
                let col_name = path.split('.').last().unwrap_or(path).to_string();
                (col_name, v.clone())
            })
        })
        .collect();

    // Navigate to record_path
    let array = json_navigate_path(&root, record_path).ok_or_else(|| {
        BundlebaseError::from(format!("Path '{}' not found in JSON document", record_path))
    })?;

    let records = array.as_array().ok_or_else(|| {
        let kind = match array {
            serde_json::Value::Object(_) => "object",
            serde_json::Value::Null => "null",
            serde_json::Value::Bool(_) => "boolean",
            serde_json::Value::Number(_) => "number",
            serde_json::Value::String(_) => "string",
            serde_json::Value::Array(_) => "array",
        };
        BundlebaseError::from(format!(
            "Expected JSON array at path '{}', got: {}",
            record_path, kind
        ))
    })?;

    if records.is_empty() {
        return Err("JSON array is empty".into());
    }

    // Flatten each record and inject meta values, then serialize to JSONL for Arrow inference
    let mut jsonl = Vec::new();
    for record in records {
        let mut flat = serde_json::Map::new();
        json_flatten_value(record, "", sep, &mut flat);
        for (col_name, meta_value) in &meta_values {
            flat.insert(col_name.clone(), meta_value.clone());
        }
        serde_json::to_writer(&mut jsonl, &flat)
            .map_err(|e| BundlebaseError::from(format!("JSON serialization error: {}", e)))?;
        jsonl.push(b'\n');
    }

    // Infer Arrow schema from the JSONL
    let (inferred, _) = arrow::json::reader::infer_json_schema(&mut Cursor::new(&jsonl), None)
        .map_err(|e| BundlebaseError::from(format!("Schema inference failed: {}", e)))?;
    let schema = Arc::new(inferred);

    // Build RecordBatches
    let batches: Vec<arrow::record_batch::RecordBatch> =
        arrow::json::ReaderBuilder::new(schema.clone())
            .build(Cursor::new(&jsonl))
            .map_err(|e| BundlebaseError::from(format!("JSON reader error: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| BundlebaseError::from(format!("JSON read error: {}", e)))?;

    // Write to Parquet
    let mut buffer = Vec::new();
    {
        let props = parquet::file::properties::WriterProperties::builder()
            .set_compression(parquet::basic::Compression::ZSTD(
                parquet::basic::ZstdLevel::try_new(3)
                    .unwrap_or(parquet::basic::ZstdLevel::default()),
            ))
            .set_max_row_group_size(128 * 1024)
            .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Chunk)
            .set_bloom_filter_enabled(true)
            .build();
        let mut writer =
            parquet::arrow::ArrowWriter::try_new(&mut buffer, schema, Some(props))
                .map_err(|e| BundlebaseError::from(format!("Failed to create Parquet writer: {}", e)))?;
        for batch in &batches {
            writer
                .write(batch)
                .map_err(|e| BundlebaseError::from(format!("Failed to write batch: {}", e)))?;
        }
        writer
            .close()
            .map_err(|e| BundlebaseError::from(format!("Failed to close Parquet writer: {}", e)))?;
    }

    Ok(Bytes::from(buffer))
}

/// Navigate a dot-notation path within a JSON value. An empty path returns the value itself.
fn json_navigate_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    if path.is_empty() {
        return Some(value);
    }
    path.split('.').try_fold(value, |current, key| current.get(key))
}

/// Recursively flatten a JSON value into a flat map using the given separator.
///
/// Nested objects become `parent<sep>child` keys. Non-object values are stored as-is.
fn json_flatten_value(
    value: &serde_json::Value,
    prefix: &str,
    sep: &str,
    out: &mut serde_json::Map<String, serde_json::Value>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let new_key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}{}{}", prefix, sep, k)
                };
                json_flatten_value(v, &new_key, sep, out);
            }
        }
        other => {
            out.insert(prefix.to_string(), other.clone());
        }
    }
}

/// Convert raw bytes in a known format to Parquet bytes.
///
/// This is the pluggable extension point for format conversion. To add support
/// for a new input format, add a match arm here.
pub fn convert_to_parquet(data: &[u8], format: &crate::connector::SourceFormat) -> Result<Bytes, BundlebaseError> {
    use crate::connector::SourceFormat;
    match format {
        SourceFormat::Parquet => Ok(Bytes::copy_from_slice(data)),
        SourceFormat::Csv => csv_to_parquet(data, b','),
        SourceFormat::Tsv => csv_to_parquet(data, b'\t'),
        SourceFormat::JsonL => jsonl_to_parquet(data),
        SourceFormat::Json => json_array_to_parquet(data),
        SourceFormat::Xlsx | SourceFormat::Xls | SourceFormat::Ods => crate::excel::excel_to_parquet(data, None),
        other => Err(format!(
            "Cannot convert format '{}' to Parquet",
            other
        ).into()),
    }
}

/// Convert a JSON array of objects to Parquet bytes.
///
/// All values are stored as strings (Utf8). Handles ragged objects — columns
/// are accumulated as they're encountered, with nulls for missing keys.
/// Writes to Parquet in chunks to limit peak memory usage.
fn json_array_to_parquet(data: &[u8]) -> Result<Bytes, BundlebaseError> {
    use arrow::array::{ArrayRef, StringBuilder};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow::record_batch::RecordBatch;
    use std::collections::HashMap;
    use std::sync::Arc;

    // Stream-parse individual objects from the JSON array using serde's StreamDeserializer.
    // This avoids holding the entire parsed Vec<Value> in memory.
    let trimmed = {
        let s = std::str::from_utf8(data)
            .map_err(|e| BundlebaseError::from(format!("Invalid UTF-8 in JSON: {}", e)))?
            .trim();
        // Strip the outer [ ] to get a comma-separated stream of objects
        if !s.starts_with('[') || !s.ends_with(']') {
            return Err("JSON data is not an array (expected [...])".into());
        }
        &s[1..s.len() - 1]
    };

    let stream = serde_json::Deserializer::from_str(trimmed)
        .into_iter::<serde_json::Value>();

    // Accumulate column schema and row data
    let mut col_order: Vec<String> = Vec::new();
    let mut col_index: HashMap<String, usize> = HashMap::new();
    let mut col_data: Vec<Vec<Option<String>>> = Vec::new();
    let mut row_count = 0usize;

    for item in stream {
        let obj = item.map_err(|e| BundlebaseError::from(format!("Failed to parse JSON object: {}", e)))?;
        let map = match obj.as_object() {
            Some(m) => m,
            None => return Err(format!(
                "JSON array element at index {} is not an object", row_count
            ).into()),
        };

        // Register new columns, backfilling nulls for prior rows
        for key in map.keys() {
            if !col_index.contains_key(key) {
                col_index.insert(key.clone(), col_order.len());
                col_order.push(key.clone());
                col_data.push(vec![None; row_count]);
            }
        }

        // Push value or null for each column
        for col_name in &col_order {
            let value = map.get(col_name).map(|v| json_value_to_string(v));
            col_data[col_index[col_name]].push(value);
        }
        row_count += 1;
    }

    if row_count == 0 {
        return Err("JSON array is empty".into());
    }

    // Build Arrow arrays and write as Parquet
    let fields: Vec<Field> = col_order
        .iter()
        .map(|name| Field::new(name, DataType::Utf8, true))
        .collect();
    let schema = Arc::new(Schema::new(fields));

    let arrays: Vec<ArrayRef> = col_data
        .into_iter()
        .map(|values| {
            let mut builder = StringBuilder::new();
            for v in &values {
                match v {
                    Some(s) => builder.append_value(s),
                    None => builder.append_null(),
                }
            }
            Arc::new(builder.finish()) as ArrayRef
        })
        .collect();

    let batch = RecordBatch::try_new(schema.clone(), arrays)
        .map_err(|e| BundlebaseError::from(format!("Failed to create RecordBatch: {}", e)))?;

    let mut buffer = Vec::new();
    {
        let props = parquet::file::properties::WriterProperties::builder()
            .set_compression(parquet::basic::Compression::ZSTD(
                parquet::basic::ZstdLevel::try_new(3)
                    .unwrap_or(parquet::basic::ZstdLevel::default()),
            ))
            .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Chunk)
            .set_bloom_filter_enabled(true)
            .build();
        let mut writer = parquet::arrow::ArrowWriter::try_new(&mut buffer, schema, Some(props))?;
        writer.write(&batch)?;
        writer.close()?;
    }

    Ok(Bytes::from(buffer))
}

/// Convert CSV/TSV bytes to Parquet bytes.
///
/// Reads column names from the header row; all values stored as text (no type inference).
fn csv_to_parquet(data: &[u8], delimiter: u8) -> Result<Bytes, BundlebaseError> {
    use arrow::csv::reader::Format;
    use arrow::csv::ReaderBuilder;
    use std::io::Cursor;

    // Infer schema with 0 data rows — only reads the header, no type guessing.
    let fmt = Format::default().with_header(true).with_delimiter(delimiter);
    let (header_schema, _) = fmt
        .infer_schema(Cursor::new(data), Some(0))
        .map_err(|e| BundlebaseError::from(format!("CSV header read failed: {}", e)))?;
    let col_names: Vec<String> = header_schema.fields().iter().map(|f| f.name().clone()).collect();

    // Read rows as raw strings using the all-text schema.
    let schema = all_utf8_schema(&col_names);
    let mut reader = ReaderBuilder::new(schema.clone())
        .with_delimiter(delimiter)
        .with_header(true)
        .build(Cursor::new(data))
        .map_err(|e| BundlebaseError::from(format!("CSV read failed: {}", e)))?;

    let batches: Vec<_> = reader
        .collect::<Result<_, _>>()
        .map_err(|e| BundlebaseError::from(format!("CSV read error: {}", e)))?;

    record_batches_to_parquet(batches, schema)
}

/// Convert JSONL (newline-delimited JSON) bytes to Parquet bytes.
///
/// Column names are taken from the first object's keys; all values stored as text.
fn jsonl_to_parquet(data: &[u8]) -> Result<Bytes, BundlebaseError> {
    let mut col_names: Option<Vec<String>> = None;
    let mut rows: Vec<Vec<String>> = Vec::new();

    for line in data.split(|&b| b == b'\n') {
        if line.iter().all(|&b| matches!(b, b' ' | b'\t' | b'\r')) {
            continue;
        }
        let obj: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(line)
            .map_err(|e| BundlebaseError::from(format!("JSONL parse error: {}", e)))?;
        if col_names.is_none() {
            col_names = Some(obj.keys().cloned().collect());
        }
        let names = col_names.as_ref().unwrap();
        rows.push(
            names.iter()
                .map(|k| obj.get(k).map(json_value_to_string).unwrap_or_default())
                .collect(),
        );
    }

    text_rows_to_parquet(&col_names.unwrap_or_default(), rows)
}

/// Convert column names and string rows to Parquet bytes.
///
/// Used by JSONL and any other text-based format that has already resolved
/// column names and stringified values.
fn text_rows_to_parquet(col_names: &[String], rows: Vec<Vec<String>>) -> Result<Bytes, BundlebaseError> {
    use arrow::array::StringBuilder;
    use arrow::record_batch::RecordBatch;

    let schema = all_utf8_schema(col_names);
    let mut builders: Vec<StringBuilder> = (0..col_names.len()).map(|_| StringBuilder::new()).collect();

    for row in &rows {
        for (i, builder) in builders.iter_mut().enumerate() {
            builder.append_value(row.get(i).map(String::as_str).unwrap_or(""));
        }
    }

    let columns: Vec<Arc<dyn arrow::array::Array>> =
        builders.iter_mut().map(|b| Arc::new(b.finish()) as _).collect();

    let batch = RecordBatch::try_new(schema.clone(), columns)
        .map_err(|e| BundlebaseError::from(format!("RecordBatch build failed: {}", e)))?;

    record_batches_to_parquet(vec![batch], schema)
}

/// Build an all-Utf8 Arrow schema from a list of column names.
fn all_utf8_schema(col_names: &[String]) -> Arc<arrow::datatypes::Schema> {
    use arrow::datatypes::{DataType, Field, Schema};
    Arc::new(Schema::new(
        col_names.iter().map(|n| Field::new(n, DataType::Utf8, true)).collect::<Vec<_>>(),
    ))
}

/// Write a list of RecordBatches to Parquet bytes.
fn record_batches_to_parquet(
    batches: Vec<arrow::record_batch::RecordBatch>,
    schema: Arc<arrow::datatypes::Schema>,
) -> Result<Bytes, BundlebaseError> {
    use parquet::basic::Compression;
    use parquet::file::properties::WriterProperties;

    let mut buffer = Vec::new();
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            parquet::basic::ZstdLevel::try_new(3)
                .unwrap_or(parquet::basic::ZstdLevel::default()),
        ))
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Chunk)
        .set_bloom_filter_enabled(true)
        .build();
    let mut writer =
        parquet::arrow::ArrowWriter::try_new(&mut buffer, schema, Some(props))
            .map_err(|e| BundlebaseError::from(format!("Parquet writer init failed: {}", e)))?;

    for batch in batches {
        writer
            .write(&batch)
            .map_err(|e| BundlebaseError::from(format!("Parquet write error: {}", e)))?;
    }
    writer
        .close()
        .map_err(|e| BundlebaseError::from(format!("Parquet writer close failed: {}", e)))?;

    Ok(Bytes::from(buffer))
}

/// Convert a JSON value to a string representation.
pub fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        // Nested objects/arrays: serialize as JSON string
        other => other.to_string(),
    }
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
    fn test_detect_format_jsonl_object() {
        assert_eq!(detect_format_from_bytes(b"  {\"key\": \"value\"}"), Some("jsonl"));
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
    fn test_detect_format_jsonl_with_bom() {
        let mut data = vec![0xEF, 0xBB, 0xBF];
        data.extend_from_slice(b"{\"key\": 1}");
        assert_eq!(detect_format_from_bytes(&data), Some("jsonl"));
    }

    #[test]
    fn test_detect_format_empty() {
        assert_eq!(detect_format_from_bytes(b""), None);
    }

    #[test]
    fn test_detect_format_whitespace_only() {
        assert_eq!(detect_format_from_bytes(b"   \n\t  "), None);
    }

    // --- json_to_parquet_with_options tests ---

    const WRAPPED_JSON: &[u8] = br#"{"total": 4, "items": [{"id": 1, "name": "Gilbert", "info": {"score": 24}}, {"id": 2, "name": "Alexa", "info": {"score": 29}}, {"id": 3, "name": "May", "info": {"score": 14}}, {"id": 4, "name": "Deloise", "info": {"score": 19}}]}"#;
    const FLAT_JSON: &[u8] = br#"[{"id": 1, "name": "Alice"}, {"id": 2, "name": "Bob"}]"#;

    fn read_parquet_batches(bytes: &bytes::Bytes) -> Vec<arrow::record_batch::RecordBatch> {
        parquet::arrow::arrow_reader::ParquetRecordBatchReader::try_new(
            bytes.clone(), 1024,
        )
        .expect("valid parquet")
        .collect::<Result<Vec<_>, _>>()
        .expect("read batches")
    }

    #[test]
    fn test_json_to_parquet_wrapped_array_schema() {
        let result = json_to_parquet_with_options(WRAPPED_JSON, "items", "_", &[]).expect("conversion");
        let batches = read_parquet_batches(&result);
        assert!(!batches.is_empty());
        let schema = batches[0].schema();
        let col_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(col_names.contains(&"id"), "expected 'id', got: {:?}", col_names);
        assert!(col_names.contains(&"name"), "expected 'name', got: {:?}", col_names);
        assert!(col_names.contains(&"info_score"), "expected 'info_score', got: {:?}", col_names);
        assert!(!col_names.contains(&"info"), "should not have un-flattened 'info'");
    }

    #[test]
    fn test_json_to_parquet_wrapped_array_row_count() {
        let result = json_to_parquet_with_options(WRAPPED_JSON, "items", "_", &[]).expect("conversion");
        let batches = read_parquet_batches(&result);
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(4, total_rows);
    }

    #[test]
    fn test_json_to_parquet_with_meta() {
        let result = json_to_parquet_with_options(WRAPPED_JSON, "items", "_", &["total"]).expect("conversion");
        let batches = read_parquet_batches(&result);
        let schema = batches[0].schema();
        let col_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(col_names.contains(&"total"), "expected 'total' meta column, got: {:?}", col_names);
    }

    #[test]
    fn test_json_to_parquet_flat_top_level_array() {
        // Empty record_path means root is the array
        let result = json_to_parquet_with_options(FLAT_JSON, "", "_", &[]).expect("conversion");
        let batches = read_parquet_batches(&result);
        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(2, total_rows);
        let schema = batches[0].schema();
        let col_names: Vec<_> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(col_names.contains(&"id"));
        assert!(col_names.contains(&"name"));
    }

    #[test]
    fn test_json_to_parquet_data_values() {
        let result = json_to_parquet_with_options(WRAPPED_JSON, "items", "_", &[]).expect("conversion");
        let batches = read_parquet_batches(&result);
        let batch = &batches[0];
        let schema = batch.schema();
        let name_idx = schema.index_of("name").expect("name column");
        let name_col = batch.column(name_idx);
        let names: Vec<_> = (0..4)
            .map(|i| name_col.as_any().downcast_ref::<arrow::array::StringArray>().expect("StringArray").value(i))
            .collect();
        assert_eq!(vec!["Gilbert", "Alexa", "May", "Deloise"], names);
    }

    #[test]
    fn test_json_to_parquet_missing_path_error() {
        let err = json_to_parquet_with_options(WRAPPED_JSON, "nonexistent", "_", &[]);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("nonexistent"));
    }

    #[test]
    fn test_json_to_parquet_non_array_path_error() {
        // "total" is a number, not an array
        let err = json_to_parquet_with_options(WRAPPED_JSON, "total", "_", &[]);
        assert!(err.is_err());
    }

}

