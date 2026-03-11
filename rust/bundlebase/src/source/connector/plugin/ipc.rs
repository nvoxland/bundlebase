//! Built-in "ipc" connector.
//!
//! Delegates to an external subprocess via JSON-RPC 2.0 over stdin/stdout,
//! with Arrow IPC (length-prefix framed) for bulk data transfer.
//! This enables users to write connectors in any language.

use crate::source::connector::{
    ArgSpec, DiscoveredLocation, SourceData, Connector, ConnectorSignature,
};
use crate::source::connector_utils;
use crate::bundle_config::is_external_code_allowed;
use crate::{BundleConfig, BundlebaseError};
use url::Url;
use arrow::ipc::reader::StreamReader;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};


// ---------------------------------------------------------------------------
// JSON-RPC types
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
// Call string parser
// ---------------------------------------------------------------------------

/// Parse a call string into a command + arguments vector.
///
/// Supported formats:
/// - `python:script.py`    → `["python", "script.py"]`
/// - `java:my.jar`         → `["java", "-jar", "my.jar"]`
/// - `docker:image`        → `["docker", "run", "-i", "--rm", "image"]`
/// - `my-command arg1 arg2` → `["my-command", "arg1", "arg2"]` (whitespace split)
fn parse_call(call: &str) -> Result<Vec<String>, BundlebaseError> {
    let call = call.trim();
    if call.is_empty() {
        return Err("'call' argument must not be empty".into());
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
            return Err("'call' argument must not be empty".into());
        }
        Ok(parts)
    }
}

// ---------------------------------------------------------------------------
// SubprocessHandle
// ---------------------------------------------------------------------------

struct SubprocessHandle {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl SubprocessHandle {
    /// Spawn a subprocess with piped stdin/stdout and inherited stderr.
    fn spawn(command: &[String]) -> Result<Self, BundlebaseError> {
        if command.is_empty() {
            return Err("Cannot spawn subprocess with empty command".into());
        }

        log::debug!("Spawning ipc source subprocess: {:?}", command);

        let mut cmd = Command::new(&command[0]);
        cmd.args(&command[1..])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn ipc source process '{}': {}", command[0], e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or("Failed to capture stdin of ipc source process")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Failed to capture stdout of ipc source process")?;

        log::debug!("IPC source subprocess spawned successfully");

        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    /// Perform protocol version handshake with the subprocess.
    ///
    /// If the server responds with protocol_version, logs it at info level.
    /// If the server returns method_not_found (-32601), logs a warning and proceeds.
    async fn perform_handshake(&mut self) -> Result<(), BundlebaseError> {
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
            .await
            .map_err(|e| format!("Failed to write handshake request: {}", e))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush handshake request: {}", e))?;

        let mut response_line = String::new();
        let bytes_read = self
            .stdout
            .read_line(&mut response_line)
            .await
            .map_err(|e| format!("Failed to read handshake response: {}", e))?;

        if bytes_read == 0 {
            log::warn!("IPC source process closed stdout during handshake, proceeding without version check");
            return Ok(());
        }

        let response: JsonRpcResponse =
            serde_json::from_str(response_line.trim()).map_err(|e| {
                format!("Failed to parse handshake response: {} (raw: {})", e, response_line.trim())
            })?;

        if let Some(err) = response.error {
            if err.code == -32601 {
                log::warn!("IPC source subprocess does not support handshake (method_not_found), proceeding without version check");
            } else {
                log::warn!("IPC source subprocess handshake returned error (code {}): {}", err.code, err.message);
            }
            return Ok(());
        }

        if let Some(result) = response.result {
            if let Some(version) = result.get("protocol_version").and_then(|v| v.as_str()) {
                log::info!("IPC source subprocess handshake successful, protocol_version={}", version);
            }
        }

        Ok(())
    }

    /// Send a JSON-RPC request and read the response.
    async fn send_request(
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

        // Write request as a single JSON line
        let mut line = serde_json::to_string(&request)
            .map_err(|e| format!("Failed to serialize JSON-RPC request: {}", e))?;
        line.push('\n');

        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("Failed to write to ipc source process stdin: {}", e))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("Failed to flush ipc source process stdin: {}", e))?;

        // Read response line
        let mut response_line = String::new();
        let bytes_read = self
            .stdout
            .read_line(&mut response_line)
            .await
            .map_err(|e| format!("Failed to read from ipc source process stdout: {}", e))?;

        if bytes_read == 0 {
            return Err("IPC source process closed stdout unexpectedly".into());
        }

        let response: JsonRpcResponse = serde_json::from_str(response_line.trim())
            .map_err(|e| format!("Failed to parse JSON-RPC response for '{}': {} (raw: {})", method, e, response_line.trim()))?;

        if response.id != id {
            return Err(format!(
                "JSON-RPC response id mismatch: expected {}, got {}",
                id, response.id
            )
            .into());
        }

        if let Some(err) = response.error {
            return Err(format!(
                "IPC source process error (code {}): {}",
                err.code, err.message
            )
            .into());
        }

        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    /// Read length-prefixed Arrow IPC data from stdout, returning the raw IPC bytes.
    ///
    /// Protocol: 4-byte big-endian u32 length, then exactly that many bytes of Arrow IPC stream.
    /// Returns `None` if the length prefix is zero (no data).
    async fn read_arrow_ipc_data(&mut self) -> Result<Option<Vec<u8>>, BundlebaseError> {
        // Read 4-byte length prefix
        let mut len_buf = [0u8; 4];
        self.stdout
            .read_exact(&mut len_buf)
            .await
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

        // Read exactly data_len bytes of IPC data
        let mut data = vec![0u8; data_len];
        self.stdout
            .read_exact(&mut data)
            .await
            .map_err(|e| format!("Failed to read Arrow IPC data ({} bytes): {}", data_len, e))?;

        Ok(Some(data))
    }
}


// ---------------------------------------------------------------------------
// IpcConnector
// ---------------------------------------------------------------------------

/// Built-in "ipc" connector that delegates to an external subprocess.
///
/// Each instance holds a subprocess handle that stays alive for the full fetch cycle.
/// The subprocess communicates via JSON-RPC 2.0 over stdin/stdout.
pub struct IpcConnector {
    handle: tokio::sync::Mutex<Option<SubprocessHandle>>,
}

impl IpcConnector {
    /// Create a new IpcConnector (no subprocess spawned yet).
    pub fn new() -> Self {
        Self {
            handle: tokio::sync::Mutex::new(None),
        }
    }

    /// Ensure the subprocess is spawned, using the `call` arg.
    async fn ensure_spawned(
        &self,
        args: &HashMap<String, String>,
    ) -> Result<(), BundlebaseError> {
        let mut guard = self.handle.lock().await;
        if guard.is_none() {
            let call = connector_utils::require_arg(args, "call", "ipc")?;
            let command = parse_call(call)?;
            let mut handle = SubprocessHandle::spawn(&command)?;
            handle.perform_handshake().await?;
            *guard = Some(handle);
        }
        Ok(())
    }
}

impl Drop for IpcConnector {
    fn drop(&mut self) {
        // Best-effort kill if the subprocess is still running
        if let Ok(mut guard) = self.handle.try_lock() {
            if let Some(ref mut handle) = *guard {
                log::debug!("Killing ipc source subprocess during drop");
                let _ = handle.child.start_kill();
            }
        } else {
            log::warn!("Could not acquire lock to kill ipc source subprocess during drop");
        }
    }
}

#[async_trait]
impl Connector for IpcConnector {
    fn signature(&self) -> ConnectorSignature {
        ConnectorSignature {
            name: "ipc".to_string(),
            arg_specs: vec![
                ArgSpec {
                    name: "copy",
                    description: "Whether to copy data into the bundle (default: true)",
                    required: false,
                    default: Some("true"),
                },
            ],
            // call is injected by source definition resolution; user kwargs pass through
            accepts_extra_args: true,
        }
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        if !is_external_code_allowed(_config)? {
            return Err("External code execution is disabled. Set system.allow_external_code=true to enable IPC sources.".into());
        }
        self.ensure_spawned(args).await?;

        // Build params: pass all args except "call" and "copy",
        // plus attached_locations so the subprocess can optimize discovery.
        let filtered_args: HashMap<String, String> = args
            .iter()
            .filter(|(k, _)| k.as_str() != "call" && k.as_str() != "copy")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let mut params = serde_json::to_value(&filtered_args)
            .map_err(|e| format!("Failed to serialize discover params: {}", e))?;
        params["attached_locations"] =
            serde_json::to_value(attached_locations)
                .map_err(|e| format!("Failed to serialize attached_locations: {}", e))?;

        let mut guard = self.handle.lock().await;
        let handle = guard
            .as_mut()
            .ok_or("IPC source subprocess not initialized")?;

        let result = handle
            .send_request("discover", params)
            .await?;

        // Parse response: { "locations": [...] }
        let locations = result
            .get("locations")
            .ok_or("discover response missing 'locations' field")?;

        let locations: Vec<serde_json::Value> = serde_json::from_value(locations.clone())
            .map_err(|e| format!("Failed to parse discover locations: {}", e))?;

        let mut discovered = Vec::with_capacity(locations.len());
        for loc in &locations {
            let location = loc
                .get("location")
                .and_then(|v| v.as_str())
                .ok_or("discover location missing 'location' field")?
                .to_string();
            let must_copy = loc
                .get("must_copy")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let format = loc
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("parquet")
                .to_string();
            let version = loc
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            discovered.push(DiscoveredLocation {
                location,
                must_copy,
                format,
                version,
            });
        }

        Ok(discovered)
    }

    async fn data(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Option<SourceData>, BundlebaseError> {
        if !is_external_code_allowed(_config)? {
            return Err("External code execution is disabled. Set system.allow_external_code=true to enable IPC sources.".into());
        }
        self.ensure_spawned(args).await?;

        let filtered_args: HashMap<String, String> = args
            .iter()
            .filter(|(k, _)| k.as_str() != "call" && k.as_str() != "copy")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let mut params = serde_json::to_value(&filtered_args)
            .map_err(|e| format!("Failed to serialize data params: {}", e))?;
        params["location"] = serde_json::json!({
            "location": location.location,
            "must_copy": location.must_copy,
            "format": location.format,
            "version": location.version,
        });

        let mut guard = self.handle.lock().await;
        let handle = guard
            .as_mut()
            .ok_or("IPC source subprocess not initialized")?;

        // Send data request, then read Arrow IPC frame from stdout.
        // Subprocess sends a zero-length frame if there's no data.
        handle.send_request("data", params).await?;

        let ipc_data = match handle.read_arrow_ipc_data().await? {
            Some(data) => data,
            None => return Ok(None),
        };

        // Parse batches lazily from the IPC blob and wrap as a stream.
        // The reader iterates over batches from the in-memory buffer without
        // collecting them all at once.
        let cursor = std::io::Cursor::new(ipc_data);
        let reader = StreamReader::try_new(cursor, None)
            .map_err(|e| format!("Failed to create Arrow IPC stream reader: {}", e))?;

        let batch_stream = Box::pin(futures::stream::iter(reader.map(|batch_result| {
            batch_result
                .map_err(|e| format!("Failed to read Arrow IPC record batch: {}", e).into())
        })));
        Ok(Some(SourceData::Arrow(batch_stream)))
    }

    async fn stable_url(
        &self,
        _location: &DiscoveredLocation,
        _args: &HashMap<String, String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Option<Url>, BundlebaseError> {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle_config::{PassedBundleConfig, Scope};
    use std::path::PathBuf;

    /// Create a BundleConfig with allow_external_code=true for tests.
    fn test_config() -> Arc<BundleConfig> {
        let mut passed = PassedBundleConfig::new();
        passed.set(&Scope::try_from("system").expect("valid scope"), "allow_external_code", "true");
        Arc::new(BundleConfig::new(Some(&passed)).expect("test config creation"))
    }

    // --- parse_call tests ---

    #[test]
    fn test_parse_call_bare() {
        let result = parse_call("my-command arg1 arg2").expect("bare command should parse");
        assert_eq!(result, vec!["my-command", "arg1", "arg2"]);
    }

    #[test]
    fn test_parse_call_python() {
        let result = parse_call("python:script.py").expect("python: prefix should parse");
        assert_eq!(result, vec!["python", "script.py"]);
    }

    #[test]
    fn test_parse_call_docker() {
        let result = parse_call("docker:my-image").expect("docker: prefix should parse");
        assert_eq!(
            result,
            vec!["docker", "run", "-i", "--rm", "my-image"]
        );
    }

    #[test]
    fn test_parse_call_empty() {
        let result = parse_call("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_call_python_empty_script() {
        let result = parse_call("python:");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_call_java() {
        let result = parse_call("java:my-source.jar").expect("java: prefix should parse");
        assert_eq!(result, vec!["java", "-jar", "my-source.jar"]);
    }

    #[test]
    fn test_parse_call_java_empty_jar() {
        let result = parse_call("java:");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_call_docker_empty_image() {
        let result = parse_call("docker:");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_call_whitespace_only() {
        let result = parse_call("   ");
        assert!(result.is_err());
    }

    // --- Signature tests ---

    #[test]
    fn test_ipc_signature() {
        let func = IpcConnector::new();
        let sig = func.signature();
        assert_eq!(sig.name, "ipc");
        // call is no longer in arg_specs — it's injected by source definition resolution
        assert_eq!(sig.arg_specs.len(), 1);
        assert_eq!(sig.arg_specs[0].name, "copy");
        assert!(!sig.arg_specs[0].required);
        assert!(sig.accepts_extra_args);
    }

    // --- External code gate tests ---

    #[tokio::test]
    async fn test_discover_blocked_when_external_code_disabled() {
        let func = IpcConnector::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "echo hello".to_string());
        let config = Arc::new(BundleConfig::new(None).expect("test config creation"));

        let result = func.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("External code execution is disabled"));
    }

    #[tokio::test]
    async fn test_data_blocked_when_external_code_disabled() {
        let func = IpcConnector::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "echo hello".to_string());
        let config = Arc::new(BundleConfig::new(None).expect("test config creation"));
        let location = DiscoveredLocation {
            location: "test.parquet".to_string(),
            must_copy: true,
            format: "parquet".to_string(),
            version: "v1".to_string(),
        };

        let result = func.data(&location, &args, &config).await;
        let err = result.err().expect("should fail");
        assert!(err.to_string().contains("External code execution is disabled"));
    }

    // --- Integration tests with mock subprocess ---

    fn mock_script_path() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("tests/fixtures/custom_source_mock.py");
        path
    }

    fn poetry_python() -> Option<String> {
        // Use poetry's virtualenv python so pyarrow and bundlebase_sdk are available.
        let output = std::process::Command::new("poetry")
            .args(["env", "info", "--executable"])
            .current_dir(env!("CARGO_MANIFEST_DIR").to_string() + "/../..")
            .output()
            .ok()?;
        let python = String::from_utf8(output.stdout).ok()?.trim().to_string();
        if python.is_empty() { None } else { Some(python) }
    }

    fn make_ipc_args() -> Option<HashMap<String, String>> {
        let python = poetry_python()?;
        let mut args = HashMap::new();
        args.insert(
            "call".to_string(),
            format!("{} {}", python, mock_script_path().display()),
        );
        Some(args)
    }

    #[tokio::test]
    async fn test_discover_via_subprocess() {
        let args = match make_ipc_args() {
            Some(a) => a,
            None => { eprintln!("Skipping: poetry not available"); return; }
        };
        let func = IpcConnector::new();
        let config = test_config();

        let locations = func
            .discover(&args, &HashSet::new(), &config)
            .await
            .expect("discover should succeed");

        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].location, "test_file_1.parquet");
        assert_eq!(locations[0].format, "parquet");
        assert_eq!(locations[0].version, "v1");
        assert!(locations[0].must_copy);
        assert_eq!(locations[1].location, "test_file_2.parquet");
    }

    #[tokio::test]
    async fn test_data_returns_arrow_batches() {
        let args = match make_ipc_args() {
            Some(a) => a,
            None => { eprintln!("Skipping: poetry not available"); return; }
        };
        let func = IpcConnector::new();
        let config = test_config();

        // Discover first to spawn the subprocess
        let locations = func
            .discover(&args, &HashSet::new(), &config)
            .await
            .expect("discover should succeed");

        // Fetch data for the first location
        let data = func
            .data(&locations[0], &args, &config)
            .await
            .expect("data should succeed");

        assert!(data.is_some());
        let source_data = data.expect("data should be Some");
        match source_data {
            SourceData::Arrow(batch_stream) => {
                let bytes = connector_utils::record_batch_stream_to_parquet(batch_stream)
                    .await
                    .expect("parquet conversion should succeed");
                assert_eq!(&bytes[..4], b"PAR1");
            }
            SourceData::RawBytes(_) => panic!("Expected Arrow data, got RawBytes"),
        }
    }

    #[tokio::test]
    async fn test_data_multi_batch_streaming() {
        use futures::StreamExt;

        let args = match make_ipc_args() {
            Some(a) => a,
            None => { eprintln!("Skipping: poetry not available"); return; }
        };
        // test_file_1 sends 2 batches; verify they're all present
        let func = IpcConnector::new();
        let config = test_config();

        let locations = func
            .discover(&args, &HashSet::new(), &config)
            .await
            .expect("discover should succeed");

        let data = func
            .data(&locations[0], &args, &config)
            .await
            .expect("data should succeed");

        assert!(data.is_some());
        let source_data = data.expect("data should be Some");
        match source_data {
            SourceData::Arrow(mut batch_stream) => {
                // Collect all batches and sum rows
                let mut total_rows = 0;
                while let Some(batch_result) = batch_stream.next().await {
                    let batch = batch_result.expect("batch should be Ok");
                    total_rows += batch.num_rows();
                }
                // IPC sends 2 batches ([1,2] + [3]) = 3 rows total
                assert_eq!(total_rows, 3);
            }
            SourceData::RawBytes(_) => panic!("Expected Arrow data, got RawBytes"),
        }
    }

    #[tokio::test]
    async fn test_stable_url_none() {
        let args = match make_ipc_args() {
            Some(a) => a,
            None => { eprintln!("Skipping: poetry not available"); return; }
        };
        let func = IpcConnector::new();
        let config = test_config();

        // Discover first to spawn the subprocess
        let locations = func
            .discover(&args, &HashSet::new(), &config)
            .await
            .expect("discover should succeed");

        let url = func
            .stable_url(&locations[0], &args, &config)
            .await
            .expect("stable_url should succeed");

        assert!(url.is_none());
    }

    #[tokio::test]
    async fn test_subprocess_error_handling() {
        let args = match make_ipc_args() {
            Some(a) => a,
            None => { eprintln!("Skipping: poetry not available"); return; }
        };
        let func = IpcConnector::new();
        let config = test_config();

        // Discover first
        func.discover(&args, &HashSet::new(), &config)
            .await
            .expect("discover should succeed");

        // Send an unknown method — the mock returns a JSON-RPC error
        let mut guard = func.handle.lock().await;
        let handle = guard.as_mut().expect("subprocess should be initialized");
        let result = handle
            .send_request("nonexistent_method", serde_json::json!({}))
            .await;

        assert!(result.is_err());
        let err_msg = result.expect_err("unknown method should return error").to_string();
        assert!(err_msg.contains("Method not found"));
    }

    // --- Go SDK integration tests ---

    fn go_binary_path() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("tests/fixtures/go_test_source");
        path
    }

    fn make_go_ipc_args() -> HashMap<String, String> {
        let mut args = HashMap::new();
        args.insert(
            "call".to_string(),
            go_binary_path().display().to_string(),
        );
        args
    }

    #[tokio::test]
    async fn test_discover_via_go_subprocess() {
        let binary = go_binary_path();
        if !binary.exists() {
            eprintln!("Skipping Go integration test: binary not found at {:?}", binary);
            return;
        }
        let func = IpcConnector::new();
        let args = make_go_ipc_args();
        let config = test_config();

        let locations = func
            .discover(&args, &HashSet::new(), &config)
            .await
            .expect("discover should succeed");

        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].location, "test_file_1.parquet");
        assert_eq!(locations[0].format, "parquet");
        assert_eq!(locations[0].version, "v1");
        assert!(locations[0].must_copy);
        assert_eq!(locations[1].location, "test_file_2.parquet");
    }

    #[tokio::test]
    async fn test_data_via_go_subprocess() {
        let binary = go_binary_path();
        if !binary.exists() {
            eprintln!("Skipping Go integration test: binary not found at {:?}", binary);
            return;
        }
        use futures::StreamExt;

        let func = IpcConnector::new();
        let args = make_go_ipc_args();
        let config = test_config();

        let locations = func
            .discover(&args, &HashSet::new(), &config)
            .await
            .expect("discover should succeed");

        let data = func
            .data(&locations[0], &args, &config)
            .await
            .expect("data should succeed");

        assert!(data.is_some());
        match data.expect("data should be Some") {
            SourceData::Arrow(mut batch_stream) => {
                let mut total_rows = 0;
                while let Some(batch_result) = batch_stream.next().await {
                    let batch = batch_result.expect("batch should be Ok");
                    total_rows += batch.num_rows();
                }
                assert_eq!(total_rows, 3);
            }
            SourceData::RawBytes(_) => panic!("Expected Arrow data, got RawBytes"),
        }
    }

    // --- Rust SDK integration tests ---

    fn rust_binary_path() -> PathBuf {
        // The example binary is built in the workspace target directory
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop(); // up from rust/bundlebase to rust/
        path.pop(); // up from rust/ to workspace root
        path.push("target/debug/examples/test_source");
        path
    }

    fn make_rust_ipc_args() -> HashMap<String, String> {
        let mut args = HashMap::new();
        args.insert(
            "call".to_string(),
            rust_binary_path().display().to_string(),
        );
        args
    }

    #[tokio::test]
    async fn test_discover_via_rust_subprocess() {
        let binary = rust_binary_path();
        if !binary.exists() {
            eprintln!(
                "Skipping Rust SDK integration test: binary not found at {:?}. \
                 Build with: cargo build --example test_source -p bundlebase-sdk",
                binary
            );
            return;
        }
        let func = IpcConnector::new();
        let args = make_rust_ipc_args();
        let config = test_config();

        let locations = func
            .discover(&args, &HashSet::new(), &config)
            .await
            .expect("discover should succeed");

        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].location, "test_file_1.parquet");
        assert_eq!(locations[0].format, "parquet");
        assert_eq!(locations[0].version, "v1");
        assert!(locations[0].must_copy);
        assert_eq!(locations[1].location, "test_file_2.parquet");
    }

    #[tokio::test]
    async fn test_data_via_rust_subprocess() {
        let binary = rust_binary_path();
        if !binary.exists() {
            eprintln!(
                "Skipping Rust SDK integration test: binary not found at {:?}. \
                 Build with: cargo build --example test_source -p bundlebase-sdk",
                binary
            );
            return;
        }
        use futures::StreamExt;

        let func = IpcConnector::new();
        let args = make_rust_ipc_args();
        let config = test_config();

        let locations = func
            .discover(&args, &HashSet::new(), &config)
            .await
            .expect("discover should succeed");

        let data = func
            .data(&locations[0], &args, &config)
            .await
            .expect("data should succeed");

        assert!(data.is_some());
        match data.expect("data should be Some") {
            SourceData::Arrow(mut batch_stream) => {
                let mut total_rows = 0;
                while let Some(batch_result) = batch_stream.next().await {
                    let batch = batch_result.expect("batch should be Ok");
                    total_rows += batch.num_rows();
                }
                assert_eq!(total_rows, 3);
            }
            SourceData::RawBytes(_) => panic!("Expected Arrow data, got RawBytes"),
        }
    }
}
