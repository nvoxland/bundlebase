use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;

use crate::types::Location;

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: Option<String>,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse<'a> {
    jsonrpc: &'static str,
    id: &'a serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError<'a>>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError<'a> {
    code: i32,
    message: &'a str,
}

// ---------------------------------------------------------------------------
// Response writers
// ---------------------------------------------------------------------------

pub(crate) fn write_response(
    w: &mut dyn Write,
    id: &serde_json::Value,
    result: serde_json::Value,
) -> std::io::Result<()> {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    };
    let mut data =
        serde_json::to_vec(&resp).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    data.push(b'\n');
    w.write_all(&data)
}

pub(crate) fn write_error(
    w: &mut dyn Write,
    id: &serde_json::Value,
    code: i32,
    message: &str,
) -> std::io::Result<()> {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError { code, message }),
    };
    let mut data =
        serde_json::to_vec(&resp).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    data.push(b'\n');
    w.write_all(&data)
}

/// Write length-prefixed Arrow IPC stream bytes.
/// An empty/None slice writes a zero-length frame.
pub(crate) fn write_arrow_ipc(
    w: &mut dyn Write,
    batches: Option<&[RecordBatch]>,
) -> std::io::Result<()> {
    let batches = match batches {
        Some(b) if !b.is_empty() => b,
        _ => {
            return w.write_all(&0u32.to_be_bytes());
        }
    };

    let mut buf = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut buf, &batches[0].schema())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        for batch in batches {
            writer
                .write(batch)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        }
        writer
            .finish()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    }

    let len = buf.len() as u32;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&buf)
}

// ---------------------------------------------------------------------------
// Param parsing helpers
// ---------------------------------------------------------------------------

/// Extract a `Vec<String>` from a JSON value (expected to be an array of strings).
pub(crate) fn parse_string_slice(v: Option<&serde_json::Value>) -> Vec<String> {
    match v {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|item| item.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

/// Extract string-only key-value pairs from params, excluding specified keys.
pub(crate) fn parse_string_map(
    params: &serde_json::Map<String, serde_json::Value>,
    exclude: &[&str],
) -> HashMap<String, String> {
    params
        .iter()
        .filter(|(k, _)| !exclude.contains(&k.as_str()))
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

/// Parse a Location from a JSON value.
pub(crate) fn parse_location(v: Option<&serde_json::Value>) -> Location {
    match v {
        Some(val) => serde_json::from_value(val.clone()).unwrap_or_else(|_| Location::new("")),
        None => Location::new(""),
    }
}
