//! IPC bridge for invoking functions via external subprocesses.
//!
//! Uses JSON-RPC 2.0 + Arrow IPC protocol over stdin/stdout pipes.
//! Supports scalar and aggregate functions via `ipc`, `java`, and `docker` runtimes.
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

use arrow::array::ArrayRef;
use arrow::datatypes::{Field, Schema};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use bundlebase_common::BundlebaseError;
use dashmap::DashMap;
use datafusion::scalar::ScalarValue;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, BufWriter, Read as IoRead, Write as IoWrite};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Default timeout for IPC function operations, in seconds.
pub const DEFAULT_FUNCTION_TIMEOUT_SECS: u64 = 30;

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

/// Parse a call/entrypoint string into a command + arguments vector.
///
/// Supported formats:
/// - `python:script.py`     -> `["python", "script.py"]`
/// - `java:my.jar`          -> `["java", "-jar", "my.jar"]`
/// - `docker:image`         -> `["docker", "run", "-i", "--rm", "image"]`
/// - `my-command arg1 arg2` -> `["my-command", "arg1", "arg2"]` (whitespace split)
pub fn parse_call(call: &str) -> Result<Vec<String>, BundlebaseError> {
    let call = call.trim();
    if call.is_empty() {
        return Err("Function entrypoint/call string must not be empty".into());
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
        let parts: Vec<String> = call
            .split_whitespace()
            .map(|s| {
                // Strip :symbol suffix from path components (added by wildcard import).
                // e.g., "/path/binary:double_val" → "/path/binary"
                // Only applies to tokens that look like file paths (contain / or .)
                if let Some(colon_pos) = s.rfind(':') {
                    let before = &s[..colon_pos];
                    if before.contains('/') || before.contains('.') {
                        return before.to_string();
                    }
                }
                s.to_string()
            })
            .collect();
        if parts.is_empty() {
            return Err("Function entrypoint/call string must not be empty".into());
        }
        Ok(parts)
    }
}

// ---------------------------------------------------------------------------
// Synchronous subprocess handle
// ---------------------------------------------------------------------------

pub struct SyncSubprocessHandle {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    timeout: Duration,
}

impl std::fmt::Debug for SyncSubprocessHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncSubprocessHandle")
            .field("next_id", &self.next_id)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl SyncSubprocessHandle {
    fn spawn(command: &[String], timeout: Duration) -> Result<Self, BundlebaseError> {
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

        let mut handle = Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
            timeout,
        };

        handle.perform_handshake()?;

        Ok(handle)
    }

    /// Perform protocol version handshake with the subprocess.
    ///
    /// If the server responds with protocol_version, logs it at info level.
    /// If the server returns method_not_found (-32601), logs a warning and proceeds.
    fn perform_handshake(&mut self) -> Result<(), BundlebaseError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: "handshake".to_string(),
            params: serde_json::json!({"protocol_version": "1"}),
        };

        let mut line = serde_json::to_string(&request)
            .map_err(|e| format!("Failed to serialize handshake request: {}", e))?;
        line.push('\n');

        self.stdin
            .write_all(line.as_bytes())
            .map_err(|e| format!("Failed to write handshake request: {}", e))?;
        self.stdin
            .flush()
            .map_err(|e| format!("Failed to flush handshake request: {}", e))?;

        let mut response_line = String::new();
        let bytes_read = self
            .read_line_with_timeout(&mut response_line)
            .map_err(|e| format!("Failed to read handshake response: {}", e))?;

        if bytes_read == 0 {
            log::warn!("IPC function process closed stdout during handshake, proceeding without version check");
            return Ok(());
        }

        let response: JsonRpcResponse =
            serde_json::from_str(response_line.trim()).map_err(|e| {
                format!(
                    "Failed to parse handshake response: {} (raw: {})",
                    e,
                    response_line.trim()
                )
            })?;

        if let Some(err) = response.error {
            if err.code == -32601 {
                log::warn!("IPC function subprocess does not support handshake (method_not_found), proceeding without version check");
            } else {
                log::warn!(
                    "IPC function subprocess handshake returned error (code {}): {}",
                    err.code,
                    err.message
                );
            }
            return Ok(());
        }

        if let Some(result) = response.result {
            if let Some(version) = result.get("protocol_version").and_then(|v| v.as_str()) {
                log::info!(
                    "IPC function subprocess handshake successful, protocol_version={}",
                    version
                );
            }
        }

        Ok(())
    }

    /// Read a line from stdout with a timeout.
    ///
    /// Spawns a watchdog thread that kills the subprocess if the read does not
    /// complete within the configured timeout. Returns the number of bytes read.
    fn read_line_with_timeout(&mut self, buf: &mut String) -> Result<usize, BundlebaseError> {
        let timeout = self.timeout;
        let child_id = self.child.id();

        // The done flag lets the watchdog know the read completed in time.
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_clone = Arc::clone(&done);

        let watchdog = std::thread::spawn(move || {
            // Poll the done flag in short intervals so the thread exits promptly
            // after the read completes, instead of sleeping the full timeout.
            let check_interval = Duration::from_millis(100);
            let start = std::time::Instant::now();
            while start.elapsed() < timeout {
                std::thread::sleep(check_interval);
                if done_clone.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
            }
            if !done_clone.load(std::sync::atomic::Ordering::Acquire) {
                log::warn!(
                    "IPC subprocess (pid {}) timed out after {} seconds, sending kill signal",
                    child_id,
                    timeout.as_secs()
                );
                // Safety: send SIGKILL to the subprocess. This is safe because we
                // only target our own child process identified by its PID.
                #[cfg(unix)]
                unsafe {
                    libc::kill(child_id as i32, libc::SIGKILL);
                }
                #[cfg(not(unix))]
                {
                    // On non-Unix platforms, there is no portable async kill;
                    // the blocking read will fail when the process eventually exits.
                    let _ = child_id;
                }
            }
        });

        let result = self.stdout.read_line(buf);

        // Signal the watchdog that the read completed.
        done.store(true, std::sync::atomic::Ordering::Release);

        // Detach the watchdog — it will exit within ~100ms now that done is set.
        drop(watchdog);

        let bytes_read = result.map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof
                || e.kind() == std::io::ErrorKind::BrokenPipe
                || e.to_string().contains("kill")
            {
                BundlebaseError::from(format!(
                    "IPC subprocess timed out after {} seconds. The subprocess may be stuck.",
                    timeout.as_secs()
                ))
            } else {
                BundlebaseError::from(format!("Failed to read from IPC subprocess stdout: {}", e))
            }
        })?;

        Ok(bytes_read)
    }

    /// Read exact bytes from stdout with a timeout.
    ///
    /// Uses the same watchdog pattern as `read_line_with_timeout`.
    fn read_exact_with_timeout(&mut self, buf: &mut [u8]) -> Result<(), BundlebaseError> {
        let timeout = self.timeout;
        let child_id = self.child.id();

        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_clone = Arc::clone(&done);

        let watchdog = std::thread::spawn(move || {
            let check_interval = Duration::from_millis(100);
            let start = std::time::Instant::now();
            while start.elapsed() < timeout {
                std::thread::sleep(check_interval);
                if done_clone.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
            }
            if !done_clone.load(std::sync::atomic::Ordering::Acquire) {
                log::warn!(
                    "IPC subprocess (pid {}) timed out after {} seconds during binary read, sending kill signal",
                    child_id,
                    timeout.as_secs()
                );
                #[cfg(unix)]
                unsafe {
                    libc::kill(child_id as i32, libc::SIGKILL);
                }
                #[cfg(not(unix))]
                {
                    let _ = child_id;
                }
            }
        });

        let result = self.stdout.read_exact(buf);

        done.store(true, std::sync::atomic::Ordering::Release);
        drop(watchdog);

        result.map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof
                || e.kind() == std::io::ErrorKind::BrokenPipe
            {
                BundlebaseError::from(format!(
                    "IPC subprocess timed out after {} seconds. The subprocess may be stuck.",
                    timeout.as_secs()
                ))
            } else {
                BundlebaseError::from(format!("Failed to read from IPC subprocess stdout: {}", e))
            }
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
            .read_line_with_timeout(&mut response_line)
            .map_err(|e| {
                BundlebaseError::from(format!(
                    "Failed to read from IPC function process stdout: {}",
                    e
                ))
            })?;

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
            let mut writer = StreamWriter::try_new(&mut buf, &batch.schema())
                .map_err(|e| format!("Failed to create Arrow IPC writer: {}", e))?;
            writer
                .write(batch)
                .map_err(|e| format!("Failed to write Arrow IPC batch: {}", e))?;
            writer
                .finish()
                .map_err(|e| format!("Failed to finish Arrow IPC stream: {}", e))?;
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
        self.read_exact_with_timeout(&mut len_buf)
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
        self.read_exact_with_timeout(&mut data)
            .map_err(|e| format!("Failed to read Arrow IPC data ({} bytes): {}", data_len, e))?;

        Ok(Some(data))
    }
}

impl Drop for SyncSubprocessHandle {
    fn drop(&mut self) {
        log::debug!("Killing IPC function subprocess during drop");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// Per-connection subprocess cache
// ---------------------------------------------------------------------------

/// Maximum number of live subprocesses per session. When exceeded, an arbitrary
/// entry is evicted (killed) to make room. Each subprocess uses 10-50 MB.
const MAX_SUBPROCESS_CACHE_SIZE: usize = 32;

/// Per-connection cache of IPC subprocess handles.
///
/// Each `Bundle` owns one of these so that subprocesses are scoped to the
/// connection/session and cleaned up when the `Bundle` is dropped.
///
/// Uses `DashMap` for lock-free concurrent reads; the inner `Mutex` provides
/// exclusive I/O access to each subprocess's stdin/stdout.
pub type SubprocessCache = Arc<DashMap<String, Arc<Mutex<SyncSubprocessHandle>>>>;

/// Create a new, empty subprocess cache.
pub fn new_subprocess_cache() -> SubprocessCache {
    Arc::new(DashMap::new())
}

/// Evict entries from the cache if it exceeds `MAX_SUBPROCESS_CACHE_SIZE`.
///
/// Removes arbitrary entries (not the one just inserted) to bring the cache
/// back under the limit. Removed entries drop their `SyncSubprocessHandle`,
/// which kills the subprocess.
fn evict_if_over_capacity(cache: &SubprocessCache, keep_key: &str) {
    while cache.len() > MAX_SUBPROCESS_CACHE_SIZE {
        // Find a key to evict that isn't the one we just inserted.
        let victim = cache
            .iter()
            .find(|entry| entry.key() != keep_key)
            .map(|entry| entry.key().clone());

        if let Some(key) = victim {
            log::info!(
                "Subprocess cache over limit ({}/{}), evicting '{}'",
                cache.len(),
                MAX_SUBPROCESS_CACHE_SIZE,
                key
            );
            cache.remove(&key);
        } else {
            break;
        }
    }
}

/// Normalize an entrypoint string for use as a cache key.
///
/// Extracts the command portion (before any whitespace arguments), attempts to
/// canonicalize it as a filesystem path, and reconstructs the full string.
/// Falls back to the raw entrypoint string if canonicalization fails.
fn normalize_cache_key(entrypoint: &str) -> String {
    let trimmed = entrypoint.trim();
    // Split into command and arguments at the first whitespace
    let (cmd_part, args_part) = match trimmed.split_once(char::is_whitespace) {
        Some((cmd, args)) => (cmd, Some(args)),
        None => (trimmed, None),
    };

    // Try to canonicalize the command path
    match std::fs::canonicalize(cmd_part) {
        Ok(canonical) => {
            let canonical_str = canonical.to_string_lossy();
            match args_part {
                Some(args) => format!("{} {}", canonical_str, args),
                None => canonical_str.into_owned(),
            }
        }
        Err(_) => entrypoint.to_string(),
    }
}

fn get_or_spawn_subprocess(
    cache: &SubprocessCache,
    entrypoint: &str,
    timeout: Duration,
) -> Result<Arc<Mutex<SyncSubprocessHandle>>, BundlebaseError> {
    let cache_key = normalize_cache_key(entrypoint);

    // Fast path: return existing handle without blocking other keys.
    if let Some(entry) = cache.get(&cache_key) {
        let handle = Arc::clone(entry.value());
        // Check if the subprocess has exited (crashed). If so, remove the
        // stale entry and fall through to re-spawn below.
        let mut guard = acquire_lock(&handle);
        match guard.child.try_wait() {
            Ok(Some(status)) => {
                log::warn!(
                    "IPC subprocess for '{}' exited with status {}, will re-spawn",
                    entrypoint,
                    status
                );
                drop(guard);
                drop(entry);
                cache.remove(&cache_key);
                // Fall through to spawn a new subprocess
            }
            Ok(None) => {
                // Still running — return it.
                drop(guard);
                return Ok(handle);
            }
            Err(e) => {
                log::warn!(
                    "Failed to check IPC subprocess status for '{}': {}, assuming alive",
                    entrypoint,
                    e
                );
                drop(guard);
                return Ok(handle);
            }
        }
    }

    // Slow path: spawn subprocess and insert.
    // DashMap's entry API ensures only one thread spawns per key.
    let handle = cache
        .entry(cache_key.clone())
        .or_try_insert_with(|| {
            let command = parse_call(entrypoint)?;
            let h = SyncSubprocessHandle::spawn(&command, timeout)?;
            Ok::<_, BundlebaseError>(Arc::new(Mutex::new(h)))
        })
        .map_err(|e: BundlebaseError| e)?;

    let result = Arc::clone(handle.value());
    drop(handle);

    // Evict old entries if over capacity.
    evict_if_over_capacity(cache, &cache_key);

    Ok(result)
}

/// Acquire the subprocess lock, recovering from poisoning by killing and
/// re-spawning the subprocess.
///
/// Lock poisoning occurs when a thread panics while holding the lock. The
/// subprocess may have corrupted I/O buffers from the panic, so we kill it
/// and let it be re-spawned on next use via `get_or_spawn_subprocess`.
fn acquire_lock(
    handle: &Arc<Mutex<SyncSubprocessHandle>>,
) -> std::sync::MutexGuard<'_, SyncSubprocessHandle> {
    handle.lock().unwrap_or_else(|poisoned| {
        log::warn!("IPC subprocess mutex was poisoned, killing subprocess for clean re-spawn");
        let mut guard = poisoned.into_inner();
        // Kill the potentially-corrupted subprocess so the next call to
        // get_or_spawn_subprocess will detect the exit and re-spawn.
        let _ = guard.child.kill();
        let _ = guard.child.wait();
        guard
    })
}

// ---------------------------------------------------------------------------
// Public API — Health Check
// ---------------------------------------------------------------------------

/// Timeout for health check ping/pong, in seconds.
const HEALTH_CHECK_TIMEOUT_SECS: u64 = 5;

/// Perform a health check on the IPC subprocess by sending a ping request.
///
/// Gets or spawns the subprocess, sends a JSON-RPC `ping` request, and expects
/// a response with result `"pong"`. Returns `Ok(())` if healthy, or an error
/// if the subprocess is unreachable or responds incorrectly.
pub fn ipc_health_check(cache: &SubprocessCache, entrypoint: &str) -> Result<(), BundlebaseError> {
    let timeout = Duration::from_secs(HEALTH_CHECK_TIMEOUT_SECS);
    let handle = get_or_spawn_subprocess(cache, entrypoint, timeout)?;
    let mut guard = acquire_lock(&handle);

    let result = guard
        .send_request("ping", serde_json::json!({}))
        .map_err(|e| {
            BundlebaseError::from(format!(
                "IPC health check failed for '{}': {}",
                entrypoint, e
            ))
        })?;

    if result.as_str() == Some("pong") {
        Ok(())
    } else {
        Err(BundlebaseError::from(format!(
            "IPC health check for '{}' returned unexpected result: {}",
            entrypoint, result
        )))
    }
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
    entrypoint: &str,
    function_name: &str,
    args: &[ArrayRef],
) -> Result<ArrayRef, BundlebaseError> {
    let timeout = Duration::from_secs(DEFAULT_FUNCTION_TIMEOUT_SECS);
    let handle = get_or_spawn_subprocess(cache, entrypoint, timeout)?;
    let mut guard = acquire_lock(&handle);

    // Send invoke request
    guard
        .send_request(
            "invoke",
            serde_json::json!({
                "function": function_name,
                "kind": "scalar",
            }),
        )
        .map_err(|e| timeout_context_error(function_name, timeout, e))?;

    // Build input RecordBatch from args
    let fields: Vec<Field> = args
        .iter()
        .enumerate()
        .map(|(i, arr)| Field::new(format!("arg_{}", i), arr.data_type().clone(), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));
    let input_batch = RecordBatch::try_new(schema, args.to_vec())
        .map_err(|e| format!("Failed to create input RecordBatch for IPC function: {}", e))?;

    // Write input Arrow IPC
    guard.write_arrow_ipc(&input_batch)?;

    // Read output Arrow IPC
    let ipc_data = guard
        .read_arrow_ipc()
        .map_err(|e| timeout_context_error(function_name, timeout, e))?
        .ok_or_else(|| {
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

    let batches: Vec<RecordBatch> =
        reader
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                format!(
                    "Failed to read Arrow IPC batch from function '{}': {}",
                    function_name, e
                )
            })?;

    if batches.is_empty() || batches[0].num_columns() == 0 {
        return Err(format!("IPC function '{}' returned no data columns", function_name).into());
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
    entrypoint: &str,
    function_name: &str,
) -> Result<String, BundlebaseError> {
    let timeout = Duration::from_secs(DEFAULT_FUNCTION_TIMEOUT_SECS);
    let handle = get_or_spawn_subprocess(cache, entrypoint, timeout)?;
    let mut guard = acquire_lock(&handle);

    let result = guard
        .send_request(
            "create_state",
            serde_json::json!({
                "function": function_name,
            }),
        )
        .map_err(|e| timeout_context_error(function_name, timeout, e))?;

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
    entrypoint: &str,
    function_name: &str,
    state_id: &str,
    values: &[ArrayRef],
) -> Result<(), BundlebaseError> {
    let timeout = Duration::from_secs(DEFAULT_FUNCTION_TIMEOUT_SECS);
    let handle = get_or_spawn_subprocess(cache, entrypoint, timeout)?;
    let mut guard = acquire_lock(&handle);

    guard
        .send_request(
            "accumulate",
            serde_json::json!({
                "function": function_name,
                "state_id": state_id,
            }),
        )
        .map_err(|e| timeout_context_error(function_name, timeout, e))?;

    // Build and write the input batch
    let fields: Vec<Field> = values
        .iter()
        .enumerate()
        .map(|(i, arr)| Field::new(format!("val_{}", i), arr.data_type().clone(), true))
        .collect();
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema, values.to_vec())
        .map_err(|e| format!("Failed to create batch for accumulate: {}", e))?;

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
    entrypoint: &str,
    function_name: &str,
    state_id1: &str,
    state_id2: &str,
) -> Result<String, BundlebaseError> {
    let timeout = Duration::from_secs(DEFAULT_FUNCTION_TIMEOUT_SECS);
    let handle = get_or_spawn_subprocess(cache, entrypoint, timeout)?;
    let mut guard = acquire_lock(&handle);

    let result = guard
        .send_request(
            "merge",
            serde_json::json!({
                "function": function_name,
                "state_id1": state_id1,
                "state_id2": state_id2,
            }),
        )
        .map_err(|e| timeout_context_error(function_name, timeout, e))?;

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
    entrypoint: &str,
    function_name: &str,
    state_id: &str,
    return_type: &arrow::datatypes::DataType,
) -> Result<ScalarValue, BundlebaseError> {
    let timeout = Duration::from_secs(DEFAULT_FUNCTION_TIMEOUT_SECS);
    let handle = get_or_spawn_subprocess(cache, entrypoint, timeout)?;
    let mut guard = acquire_lock(&handle);

    guard
        .send_request(
            "evaluate",
            serde_json::json!({
                "function": function_name,
                "state_id": state_id,
            }),
        )
        .map_err(|e| timeout_context_error(function_name, timeout, e))?;

    // Read Arrow IPC result
    let ipc_data = guard
        .read_arrow_ipc()
        .map_err(|e| timeout_context_error(function_name, timeout, e))?;

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

/// Add function name and timeout context to timeout-related errors.
///
/// If the error message already mentions "timed out", rewrites it to include
/// the function name. Otherwise returns the original error unchanged.
fn timeout_context_error(
    function_name: &str,
    timeout: Duration,
    error: BundlebaseError,
) -> BundlebaseError {
    let msg = error.to_string();
    if msg.contains("timed out") {
        BundlebaseError::from(format!(
            "Function '{}' timed out after {} seconds. The subprocess may be stuck.",
            function_name,
            timeout.as_secs()
        ))
    } else {
        error
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
        assert_eq!(result, vec!["docker", "run", "-i", "--rm", "my-image"]);
    }

    #[test]
    fn test_parse_call_empty() {
        assert!(parse_call("").is_err());
    }

    #[test]
    fn test_parse_call_whitespace_only() {
        assert!(parse_call("   ").is_err());
    }

    #[test]
    fn test_default_timeout_constant() {
        assert_eq!(DEFAULT_FUNCTION_TIMEOUT_SECS, 30);
    }

    #[test]
    fn test_timeout_context_error_with_timeout_message() {
        let timeout = Duration::from_secs(30);
        let err = BundlebaseError::from(
            "IPC subprocess timed out after 30 seconds. The subprocess may be stuck.".to_string(),
        );
        let result = timeout_context_error("my_func", timeout, err);
        let msg = result.to_string();
        assert!(
            msg.contains("my_func"),
            "should contain function name: {}",
            msg
        );
        assert!(
            msg.contains("30 seconds"),
            "should contain timeout: {}",
            msg
        );
    }

    #[test]
    fn test_timeout_context_error_without_timeout_message() {
        let timeout = Duration::from_secs(30);
        let err = BundlebaseError::from("some other error".to_string());
        let result = timeout_context_error("my_func", timeout, err);
        let msg = result.to_string();
        assert!(
            msg.contains("some other error"),
            "should preserve original message: {}",
            msg
        );
        assert!(
            !msg.contains("my_func"),
            "should not add function name: {}",
            msg
        );
    }
}
