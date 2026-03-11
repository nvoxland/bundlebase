//! IPC bridge for invoking functions via external subprocesses.
//!
//! Uses JSON-RPC 2.0 + Arrow IPC protocol over stdin/stdout pipes.
//! Supports scalar and aggregate functions via `ipc`, `java`, and `docker` runners.
//!
//! Protocol:
//! - **Scalar invoke**: JSON-RPC `invoke` request, then Arrow IPC input/output.
//! - **Aggregate create_state**: JSON-RPC `create_state` request → returns state ID.
//! - **Aggregate accumulate**: JSON-RPC `accumulate` request with state ID, then
//!   Arrow IPC input batch. State updated server-side.
//! - **Aggregate merge**: JSON-RPC `merge` request with two state IDs → returns
//!   merged state ID.
//! - **Aggregate evaluate**: JSON-RPC `evaluate` request with state ID → returns
//!   result as Arrow IPC.

use crate::BundlebaseError;
use arrow::array::ArrayRef;
use arrow::datatypes::{Field, Schema};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use datafusion::scalar::ScalarValue;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Read as IoRead, Write as IoWrite};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// JSON-RPC types (sync, for use in DataFusion's invoke path)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: u64,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

// ---------------------------------------------------------------------------
// Call string parser (duplicated from connector ipc.rs for independence)
// ---------------------------------------------------------------------------

/// Parse a call/logic string into a command + arguments vector.
///
/// Supported formats:
/// - `python:script.py`     -> `["python", "script.py"]`
/// - `java:my.jar`          -> `["java", "-jar", "my.jar"]`
/// - `docker:image`         -> `["docker", "run", "-i", "--rm", "image"]`
/// - `my-command arg1 arg2` -> `["my-command", "arg1", "arg2"]` (whitespace split)
fn parse_call(call: &str) -> Result<Vec<String>, BundlebaseError> {
    let call = call.trim();
    if call.is_empty() {
        return Err("Function logic/call string must not be empty".into());
    }

    if let Some(script) = call.strip_prefix("python:") {
        let script = script.trim();
        if script.is_empty() {
            return Err("python: call requires a script path".into());
        }
        Ok(vec!["python".to_string(), script.to_string()])
    } else if let Some(jar) = call.strip_prefix("java:") {
        let jar = jar.trim();
        if jar.is_empty() {
            return Err("java: call requires a JAR path".into());
        }
        Ok(vec![
            "java".to_string(),
            "-jar".to_string(),
            jar.to_string(),
        ])
    } else if let Some(image) = call.strip_prefix("docker:") {
        let image = image.trim();
        if image.is_empty() {
            return Err("docker: call requires an image name".into());
        }
        Ok(vec![
            "docker".to_string(),
            "run".to_string(),
            "-i".to_string(),
            "--rm".to_string(),
            image.to_string(),
        ])
    } else {
        let parts: Vec<String> = call.split_whitespace().map(|s| s.to_string()).collect();
        if parts.is_empty() {
            return Err("Function logic/call string must not be empty".into());
        }
        Ok(parts)
    }
}

// ---------------------------------------------------------------------------
// Synchronous subprocess handle
// ---------------------------------------------------------------------------

pub(crate) struct SyncSubprocessHandle {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl std::fmt::Debug for SyncSubprocessHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncSubprocessHandle")
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl SyncSubprocessHandle {
    fn spawn(command: &[String]) -> Result<Self, BundlebaseError> {
        if command.is_empty() {
            return Err("Cannot spawn subprocess with empty command".into());
        }

        log::debug!("Spawning IPC function subprocess: {:?}", command);

        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                format!(
                    "Failed to spawn IPC function process '{}': {}",
                    command[0], e
                )
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or("Failed to capture stdin of IPC function process")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Failed to capture stdout of IPC function process")?;

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, BundlebaseError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&request)
            .map_err(|e| format!("Failed to serialize JSON-RPC request: {}", e))?;
        line.push('\n');

        self.stdin
            .write_all(line.as_bytes())
            .map_err(|e| format!("Failed to write to IPC function process stdin: {}", e))?;
        self.stdin
            .flush()
            .map_err(|e| format!("Failed to flush IPC function process stdin: {}", e))?;

        let mut response_line = String::new();
        let bytes_read = self
            .stdout
            .read_line(&mut response_line)
            .map_err(|e| format!("Failed to read from IPC function process stdout: {}", e))?;

        if bytes_read == 0 {
            return Err("IPC function process closed stdout unexpectedly".into());
        }

        let response: JsonRpcResponse =
            serde_json::from_str(response_line.trim()).map_err(|e| {
                format!(
                    "Failed to parse JSON-RPC response for '{}': {} (raw: {})",
                    method,
                    e,
                    response_line.trim()
                )
            })?;

        if response.id != id {
            return Err(format!(
                "JSON-RPC response id mismatch: expected {}, got {}",
                id, response.id
            )
            .into());
        }

        if let Some(err) = response.error {
            return Err(format!(
                "IPC function process error (code {}): {}",
                err.code, err.message
            )
            .into());
        }

        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    fn write_arrow_ipc(&mut self, batch: &RecordBatch) -> Result<(), BundlebaseError> {
        let mut buf = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut buf, &batch.schema()).map_err(|e| {
                format!("Failed to create Arrow IPC writer: {}", e)
            })?;
            writer.write(batch).map_err(|e| {
                format!("Failed to write Arrow IPC batch: {}", e)
            })?;
            writer.finish().map_err(|e| {
                format!("Failed to finish Arrow IPC stream: {}", e)
            })?;
        }

        let len = buf.len() as u32;
        self.stdin
            .write_all(&len.to_be_bytes())
            .map_err(|e| format!("Failed to write Arrow IPC length prefix: {}", e))?;
        self.stdin
            .write_all(&buf)
            .map_err(|e| format!("Failed to write Arrow IPC data: {}", e))?;
        self.stdin
            .flush()
            .map_err(|e| format!("Failed to flush Arrow IPC data: {}", e))?;

        Ok(())
    }

    fn read_arrow_ipc(&mut self) -> Result<Option<Vec<u8>>, BundlebaseError> {
        let mut len_buf = [0u8; 4];
        self.stdout
            .read_exact(&mut len_buf)
            .map_err(|e| format!("Failed to read Arrow IPC length prefix: {}", e))?;
        let data_len = u32::from_be_bytes(len_buf) as usize;

        if data_len == 0 {
            return Ok(None);
        }

        const MAX_IPC_SIZE: usize = 2 * 1024 * 1024 * 1024; // 2GB
        if data_len > MAX_IPC_SIZE {
            return Err(format!(
                "Arrow IPC data too large: {} bytes (max {})",
                data_len, MAX_IPC_SIZE
            )
            .into());
        }

        let mut data = vec![0u8; data_len];
        self.stdout
            .read_exact(&mut data)
            .map_err(|e| format!("Failed to read Arrow IPC data ({} bytes): {}", data_len, e))?;

        Ok(Some(data))
    }
}

impl Drop for SyncSubprocessHandle {
    fn drop(&mut self) {
        log::debug!("Killing IPC function subprocess during drop");
        let _ = self.child.kill();
    }
}

// ---------------------------------------------------------------------------
// Per-connection subprocess cache
// ---------------------------------------------------------------------------

/// Per-connection cache of IPC subprocess handles.
///
/// Each `Bundle` owns one of these so that subprocesses are scoped to the
/// connection/session and cleaned up when the `Bundle` is dropped.
pub type SubprocessCache = Arc<Mutex<HashMap<String, Arc<Mutex<SyncSubprocessHandle>>>>>;

/// Create a new, empty subprocess cache.
pub fn new_subprocess_cache() -> SubprocessCache {
    Arc::new(Mutex::new(HashMap::new()))
}

fn get_or_spawn_subprocess(
    cache: &SubprocessCache,
    logic: &str,
) -> Result<Arc<Mutex<SyncSubprocessHandle>>, BundlebaseError> {
    let mut cache = cache.lock().map_err(|e| {
        BundlebaseError::from(format!("Failed to acquire subprocess cache lock: {}", e))
    })?;

    if let Some(handle) = cache.get(logic) {
        return Ok(Arc::clone(handle));
    }

    let command = parse_call(logic)?;
    let handle = SyncSubprocessHandle::spawn(&command)?;
    let handle = Arc::new(Mutex::new(handle));
    cache.insert(logic.to_string(), Arc::clone(&handle));
    Ok(handle)
}

// ---------------------------------------------------------------------------
// Public API — Scalar
// ---------------------------------------------------------------------------

/// Invoke a scalar function via an IPC subprocess.
///
/// The protocol:
/// 1. Send JSON-RPC `invoke` request with function name
/// 2. Write input Arrow IPC (all args as columns in one RecordBatch)
/// 3. Read output Arrow IPC (single-column RecordBatch)
/// 4. Return the single output column
pub fn invoke_ipc_scalar(
    cache: &SubprocessCache,
    logic: &str,
    function_name: &str,
    args: &[ArrayRef],
) -> Result<ArrayRef, BundlebaseError> {
    let handle = get_or_spawn_subprocess(cache, logic)?;
    let mut guard = handle.lock().map_err(|e| {
        BundlebaseError::from(format!("Failed to acquire subprocess lock: {}", e))
    })?;

    // Send invoke request
    guard.send_request(
        "invoke",
        serde_json::json!({
            "function": function_name,
            "kind": "scalar",
        }),
    )?;

    // Build input RecordBatch from args
    let fields: Vec<Field> = args
        .iter()
        .enumerate()
        .map(|(i, arr)| Field::new(format!("arg_{}", i), arr.data_type().clone(), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));
    let input_batch = RecordBatch::try_new(schema, args.to_vec()).map_err(|e| {
        format!("Failed to create input RecordBatch for IPC function: {}", e)
    })?;

    // Write input Arrow IPC
    guard.write_arrow_ipc(&input_batch)?;

    // Read output Arrow IPC
    let ipc_data = guard.read_arrow_ipc()?.ok_or_else(|| {
        BundlebaseError::from(format!(
            "IPC function '{}' returned empty output",
            function_name
        ))
    })?;

    // Parse output RecordBatch
    let cursor = std::io::Cursor::new(ipc_data);
    let reader = StreamReader::try_new(cursor, None).map_err(|e| {
        format!(
            "Failed to parse Arrow IPC output from function '{}': {}",
            function_name, e
        )
    })?;

    let batches: Vec<RecordBatch> = reader
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| {
            format!(
                "Failed to read Arrow IPC batch from function '{}': {}",
                function_name, e
            )
        })?;

    if batches.is_empty() || batches[0].num_columns() == 0 {
        return Err(format!(
            "IPC function '{}' returned no data columns",
            function_name
        )
        .into());
    }

    // Return the first (and only) column from the first batch
    Ok(Arc::clone(batches[0].column(0)))
}

// ---------------------------------------------------------------------------
// Public API — Aggregate
// ---------------------------------------------------------------------------

/// Create initial accumulator state for an aggregate function via IPC.
///
/// Protocol:
/// 1. Send JSON-RPC `create_state` with function name
/// 2. Response contains an opaque state ID (string)
///
/// State is held server-side in the subprocess.
pub fn ipc_aggregate_create_state(
    cache: &SubprocessCache,
    logic: &str,
    function_name: &str,
) -> Result<String, BundlebaseError> {
    let handle = get_or_spawn_subprocess(cache, logic)?;
    let mut guard = handle.lock().map_err(|e| {
        BundlebaseError::from(format!("Failed to acquire subprocess lock: {}", e))
    })?;

    let result = guard.send_request(
        "create_state",
        serde_json::json!({
            "function": function_name,
        }),
    )?;

    let state_id = result
        .get("state_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            BundlebaseError::from(format!(
                "IPC create_state for '{}' did not return a state_id",
                function_name
            ))
        })?
        .to_string();

    Ok(state_id)
}

/// Accumulate a batch into an aggregate state via IPC.
///
/// Protocol:
/// 1. Send JSON-RPC `accumulate` with function name and state ID
/// 2. Write Arrow IPC input batch
/// 3. State is updated server-side (no data returned)
pub fn ipc_aggregate_accumulate(
    cache: &SubprocessCache,
    logic: &str,
    function_name: &str,
    state_id: &str,
    values: &[ArrayRef],
) -> Result<(), BundlebaseError> {
    let handle = get_or_spawn_subprocess(cache, logic)?;
    let mut guard = handle.lock().map_err(|e| {
        BundlebaseError::from(format!("Failed to acquire subprocess lock: {}", e))
    })?;

    guard.send_request(
        "accumulate",
        serde_json::json!({
            "function": function_name,
            "state_id": state_id,
        }),
    )?;

    // Build and write the input batch
    let fields: Vec<Field> = values
        .iter()
        .enumerate()
        .map(|(i, arr)| Field::new(format!("val_{}", i), arr.data_type().clone(), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema, values.to_vec()).map_err(|e| {
        format!("Failed to create batch for accumulate: {}", e)
    })?;

    guard.write_arrow_ipc(&batch)?;

    Ok(())
}

/// Merge two aggregate states via IPC.
///
/// Protocol:
/// 1. Send JSON-RPC `merge` with function name and two state IDs
/// 2. Response contains the merged state ID
pub fn ipc_aggregate_merge(
    cache: &SubprocessCache,
    logic: &str,
    function_name: &str,
    state_id1: &str,
    state_id2: &str,
) -> Result<String, BundlebaseError> {
    let handle = get_or_spawn_subprocess(cache, logic)?;
    let mut guard = handle.lock().map_err(|e| {
        BundlebaseError::from(format!("Failed to acquire subprocess lock: {}", e))
    })?;

    let result = guard.send_request(
        "merge",
        serde_json::json!({
            "function": function_name,
            "state_id1": state_id1,
            "state_id2": state_id2,
        }),
    )?;

    let merged_id = result
        .get("state_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            BundlebaseError::from(format!(
                "IPC merge for '{}' did not return a state_id",
                function_name
            ))
        })?
        .to_string();

    Ok(merged_id)
}

/// Evaluate an aggregate state to produce the final result via IPC.
///
/// Protocol:
/// 1. Send JSON-RPC `evaluate` with function name and state ID
/// 2. Read Arrow IPC output (single-row, single-column RecordBatch)
/// 3. Extract ScalarValue from the result
pub fn ipc_aggregate_evaluate(
    cache: &SubprocessCache,
    logic: &str,
    function_name: &str,
    state_id: &str,
    return_type: &arrow::datatypes::DataType,
) -> Result<ScalarValue, BundlebaseError> {
    let handle = get_or_spawn_subprocess(cache, logic)?;
    let mut guard = handle.lock().map_err(|e| {
        BundlebaseError::from(format!("Failed to acquire subprocess lock: {}", e))
    })?;

    guard.send_request(
        "evaluate",
        serde_json::json!({
            "function": function_name,
            "state_id": state_id,
        }),
    )?;

    // Read Arrow IPC result
    let ipc_data = guard.read_arrow_ipc()?;

    match ipc_data {
        None => ScalarValue::try_from(return_type).map_err(|e| {
            format!(
                "Failed to create null ScalarValue for '{}': {}",
                function_name, e
            )
            .into()
        }),
        Some(data) => {
            let cursor = std::io::Cursor::new(data);
            let reader = StreamReader::try_new(cursor, None).map_err(|e| {
                format!(
                    "Failed to parse Arrow IPC from evaluate for '{}': {}",
                    function_name, e
                )
            })?;

            let batches: Vec<RecordBatch> = reader
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    format!(
                        "Failed to read evaluate result for '{}': {}",
                        function_name, e
                    )
                })?;

            if batches.is_empty() || batches[0].num_columns() == 0 || batches[0].num_rows() == 0 {
                return ScalarValue::try_from(return_type).map_err(|e| {
                    format!(
                        "Failed to create null ScalarValue for '{}': {}",
                        function_name, e
                    )
                    .into()
                });
            }

            ScalarValue::try_from_array(batches[0].column(0), 0).map_err(|e| {
                format!(
                    "Failed to extract ScalarValue from evaluate for '{}': {}",
                    function_name, e
                )
                .into()
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_call_bare() {
        let result = parse_call("my-command arg1 arg2").expect("should parse");
        assert_eq!(result, vec!["my-command", "arg1", "arg2"]);
    }

    #[test]
    fn test_parse_call_python() {
        let result = parse_call("python:script.py").expect("should parse");
        assert_eq!(result, vec!["python", "script.py"]);
    }

    #[test]
    fn test_parse_call_java() {
        let result = parse_call("java:my.jar").expect("should parse");
        assert_eq!(result, vec!["java", "-jar", "my.jar"]);
    }

    #[test]
    fn test_parse_call_docker() {
        let result = parse_call("docker:my-image").expect("should parse");
        assert_eq!(
            result,
            vec!["docker", "run", "-i", "--rm", "my-image"]
        );
    }

    #[test]
    fn test_parse_call_empty() {
        assert!(parse_call("").is_err());
    }

    #[test]
    fn test_parse_call_whitespace_only() {
        assert!(parse_call("   ").is_err());
    }
}
