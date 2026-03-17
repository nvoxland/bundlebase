//! Logic runtime behavior trait and implementations.
//!
//! Each `LogicRuntime` variant wraps a concrete struct implementing `LogicRuntimeImpl`,
//! centralizing per-runtime logic that was previously scattered across match statements.

mod python;
mod ffi;
mod ipc;
mod java;
mod docker;

pub use python::PythonRuntime;
pub use ffi::FfiRuntime;
pub use ipc::IpcRuntime;
pub use java::JavaRuntime;
pub use docker::DockerRuntime;

use async_trait::async_trait;
use crate::function::ipc_bridge::{self, SubprocessCache};
pub use crate::function::lib_bridge::{Manifest, ManifestEntry};
use crate::io::IOReadWriteDir;
use crate::BundlebaseError;
use arrow::array::ArrayRef;
use arrow::datatypes::DataType;
use datafusion::common::Result as DFResult;
use datafusion::logical_expr::{Accumulator, ColumnarValue};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// The type of connector registry used by a runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeType {
    /// Native (in-process) execution via FFI shared libraries or Python bridge.
    Native,
    /// IPC (inter-process communication) via subprocess JSON-RPC protocol.
    Ipc,
}

/// Trait for runtime-specific behavior.
///
/// Each runtime struct holds parsed fields and implements this trait,
/// so methods can use own fields directly instead of re-parsing logic strings.
#[async_trait]
pub trait LogicRuntimeImpl: Send + Sync + std::fmt::Debug {
    /// Whether this runtime's logic can be persisted in a bundle.
    fn can_bundle(&self) -> bool;

    /// The type of connector registry used by this runtime.
    fn runtime_type(&self) -> RuntimeType;

    /// Reconstruct the logic portion of the FROM string.
    fn to_logic_string(&self) -> String;

    /// Return the file path if this runtime references a local file.
    fn file_path(&self) -> Option<&str>;

    /// Build the prefixed call string for IPC/native dispatch.
    fn build_call_string(&self) -> String;

    /// Validate that the referenced logic (file, module, etc.) is reachable.
    ///
    /// Called at import time to fail fast if the logic doesn't exist.
    /// Default implementation is a no-op (e.g., Docker images are validated at run time).
    fn validate_logic(&self) -> Result<(), BundlebaseError> {
        Ok(())
    }

    /// Verify this runtime's bundled artifact is loadable (e.g., load manifest from shared lib).
    fn verify_loadable(&self) -> Result<(), BundlebaseError> {
        Ok(())
    }

    /// Load the function manifest for wildcard discovery.
    /// Returns None if this runtime doesn't support wildcard discovery.
    fn load_manifest(&self) -> Result<Option<Manifest>, BundlebaseError> {
        Ok(None)
    }

    /// Look up a single function's metadata from manifest.
    /// Default implementation loads the manifest and searches it.
    /// Runtimes that don't use manifests (e.g., Python) should override.
    fn lookup_function_in_manifest(
        &self,
        function_name: &str,
    ) -> Result<ManifestEntry, BundlebaseError> {
        let manifest = self.load_manifest()?.ok_or_else(|| -> BundlebaseError {
            format!(
                "Function discovery not supported for this runtime (logic: '{}')",
                self.to_logic_string()
            )
            .into()
        })?;
        find_in_manifest(manifest, function_name, &self.to_logic_string())
    }

    /// Invoke a scalar function.
    fn invoke_scalar(
        &self,
        name: &str,
        function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<ColumnarValue>;

    /// Create an accumulator for an aggregate function.
    fn create_accumulator(
        &self,
        name: &str,
        function_name: &str,
        return_type: &DataType,
        subprocess_cache: &SubprocessCache,
    ) -> DFResult<Box<dyn Accumulator>>;

    /// DataType for aggregate state serialization.
    /// IPC runtimes use Utf8 (opaque state ID), others use return type.
    fn aggregate_state_type(&self, return_type: &DataType) -> DataType {
        return_type.clone()
    }

    /// Copy the file referenced by this runtime into the bundle's data directory.
    ///
    /// Returns the new bundle-relative path, or `None` if no copy is needed
    /// (i.e., the runtime doesn't reference a local file).
    async fn copy_into_bundle(
        &self,
        data_dir: &Arc<dyn IOReadWriteDir>,
    ) -> Result<Option<String>, BundlebaseError> {
        let file_path = match self.file_path() {
            Some(p) => p.to_string(),
            None => return Ok(None),
        };

        let abs_path = if file_path.starts_with('/') {
            std::path::PathBuf::from(&file_path)
        } else {
            std::env::current_dir()
                .map_err(|e| {
                    BundlebaseError::from(format!("Failed to get current directory: {}", e))
                })?
                .join(&file_path)
        };

        let file_bytes = tokio::fs::read(&abs_path).await.map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to read file '{}': {}",
                abs_path.display(),
                e
            ))
        })?;

        let ext = abs_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin");

        let stream = futures::stream::once(async move {
            Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from(file_bytes))
        });
        let write_result = data_dir.write_stream(Box::pin(stream), ext).await?;

        let hash = &write_result.hash;
        let bundle_path = format!("{}/{}.{}", &hash[..2], &hash[2..16], ext);

        Ok(Some(bundle_path))
    }
}

// ==================== Shared helpers ====================

/// Check that a file-based logic path exists on disk.
///
/// Resolves relative paths against the current working directory.
/// Returns a descriptive error if the file is not found.
pub(super) fn validate_file_reachable(path: &str, label: &str) -> Result<(), BundlebaseError> {
    let abs = if path.starts_with('/') {
        std::path::PathBuf::from(path)
    } else {
        std::env::current_dir()
            .map_err(|e| BundlebaseError::from(format!("Failed to get current directory: {}", e)))?
            .join(path)
    };
    if !abs.exists() {
        return Err(format!(
            "{} not found: '{}' (resolved to '{}')",
            label,
            path,
            abs.display()
        )
        .into());
    }
    Ok(())
}

/// Look up a function by name in a manifest, or return a descriptive error.
pub(super) fn find_in_manifest(
    manifest: Manifest,
    function_name: &str,
    logic: &str,
) -> Result<ManifestEntry, BundlebaseError> {
    let available_names: Vec<String> = manifest.functions.iter().map(|e| e.name.clone()).collect();

    manifest
        .functions
        .into_iter()
        .find(|e| e.name == function_name)
        .ok_or_else(|| {
            if available_names.is_empty() {
                format!(
                    "Function '{}' not found in manifest from '{}'. \
                     The manifest contains no functions.",
                    function_name, logic
                )
            } else {
                format!(
                    "Function '{}' not found in manifest from '{}'. \
                     Available functions: {}",
                    function_name, logic, available_names.join(", ")
                )
            }
            .into()
        })
}

/// Shared IPC scalar invocation for Ipc, Java, and Docker runtimes.
pub(super) fn invoke_ipc_scalar_impl(
    name: &str,
    logic: &str,
    args: &datafusion::logical_expr::ScalarFunctionArgs,
    subprocess_cache: &SubprocessCache,
) -> DFResult<ColumnarValue> {
    let arrays: Vec<ArrayRef> = args
        .args
        .iter()
        .map(|cv| match cv {
            ColumnarValue::Array(arr) => Ok(Arc::clone(arr)),
            ColumnarValue::Scalar(scalar) => scalar
                .to_array_of_size(args.number_rows)
                .map_err(|e| datafusion::common::DataFusionError::Execution(e.to_string())),
        })
        .collect::<DFResult<Vec<_>>>()?;

    // Extract function name from the call - use the name parameter which is the display name
    // For IPC, we need to extract the actual function name from the display name (namespace.name -> name)
    let func_name = name.rsplit('.').next().unwrap_or(name);

    let result =
        ipc_bridge::invoke_ipc_scalar(subprocess_cache, logic, func_name, &arrays)
            .map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "IPC function '{}' ({}) failed: {}",
                    name, logic, e
                ))
            })?;

    Ok(ColumnarValue::Array(result))
}

/// Shared IPC accumulator creation for Ipc, Java, and Docker runtimes.
pub(super) fn create_ipc_accumulator(
    name: &str,
    logic: &str,
    function_name: &str,
    return_type: &DataType,
    subprocess_cache: &SubprocessCache,
) -> DFResult<Box<dyn Accumulator>> {
    let state_id =
        ipc_bridge::ipc_aggregate_create_state(subprocess_cache, logic, function_name)
            .map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "Failed to create IPC aggregate state for '{}': {}",
                    name, e
                ))
            })?;

    Ok(Box::new(crate::function::aggregate::IpcAccumulator {
        logic: logic.to_string(),
        function_name: function_name.to_string(),
        display_name: name.to_string(),
        state_id,
        return_type: return_type.clone(),
        subprocess_cache: Arc::clone(subprocess_cache),
    }))
}

/// The execution environment for connector/function logic.
///
/// Each variant wraps a concrete struct holding parsed fields.
/// Serializes as the FROM string (e.g., `"ffi::./mylib.so:double_val"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicRuntime {
    Python(PythonRuntime),
    Ffi(FfiRuntime),
    Java(JavaRuntime),
    Docker(DockerRuntime),
    Ipc(IpcRuntime),
}

impl LogicRuntime {
    /// Parse a FROM string like `"runtime::logic"` into a LogicRuntime.
    ///
    /// The scheme (before `::`) determines the variant, and everything after
    /// `::` is parsed by the variant's constructor.
    ///
    /// # Examples
    /// - `"ipc::./my_func"` → `LogicRuntime::Ipc(IpcRuntime { path: "./my_func" })`
    /// - `"ffi::./mylib.so:double_val"` → `LogicRuntime::Ffi(FfiRuntime { path: "./mylib.so", symbol: Some("double_val") })`
    /// - `"python::mod:func"` → `LogicRuntime::Python(PythonRuntime { module: "mod", function: "func" })`
    pub fn parse_from(from: &str) -> Result<Self, BundlebaseError> {
        let separator = "::";
        let pos = from.find(separator).ok_or_else(|| -> BundlebaseError {
            format!(
                "Invalid FROM '{}'. Expected format: 'runtime::logic' (e.g., 'ipc::./my_func').",
                from
            )
            .into()
        })?;
        let scheme = &from[..pos];
        let logic = &from[pos + separator.len()..];
        if logic.is_empty() {
            return Err(format!(
                "Invalid FROM '{}'. Logic part after '::' cannot be empty.",
                from
            )
            .into());
        }

        match scheme {
            "python" => Ok(LogicRuntime::Python(PythonRuntime::parse(logic)?)),
            "ffi" | "lib" => Ok(LogicRuntime::Ffi(FfiRuntime::parse(logic)?)),
            "java" => Ok(LogicRuntime::Java(JavaRuntime::parse(logic)?)),
            "docker" => Ok(LogicRuntime::Docker(DockerRuntime::parse(logic)?)),
            "ipc" => Ok(LogicRuntime::Ipc(IpcRuntime::parse(logic)?)),
            _ => Err(format!(
                "Invalid runtime '{}'. Must be one of: python, ffi, java, docker, ipc.",
                scheme
            )
            .into()),
        }
    }

    /// Reconstruct the full FROM string: `"runtime::logic"`.
    pub fn to_from_string(&self) -> String {
        format!("{}::{}", self.runtime_name(), self.to_logic_string())
    }

    /// Return a new LogicRuntime with a different file path.
    ///
    /// Only meaningful for runtimes that reference files (Ffi, Ipc, Java).
    /// Panics for Python and Docker (which don't have file paths).
    pub fn with_path(self, new_path: String) -> Self {
        match self {
            LogicRuntime::Ffi(r) => LogicRuntime::Ffi(r.with_path(new_path)),
            LogicRuntime::Ipc(r) => LogicRuntime::Ipc(r.with_path(new_path)),
            LogicRuntime::Java(r) => LogicRuntime::Java(r.with_path(new_path)),
            LogicRuntime::Python(_) | LogicRuntime::Docker(_) => {
                panic!("with_path not supported for {} runtime", self.runtime_name())
            }
        }
    }

    // ---- Delegate all LogicRuntimeImpl methods ----

    fn inner(&self) -> &dyn LogicRuntimeImpl {
        match self {
            LogicRuntime::Python(r) => r,
            LogicRuntime::Ffi(r) => r,
            LogicRuntime::Java(r) => r,
            LogicRuntime::Docker(r) => r,
            LogicRuntime::Ipc(r) => r,
        }
    }

    /// Whether this runtime's logic can be persisted in a bundle.
    pub fn can_bundle(&self) -> bool {
        self.inner().can_bundle()
    }

    /// The type of connector registry used by this runtime.
    pub fn runtime_type(&self) -> RuntimeType {
        self.inner().runtime_type()
    }

    /// The runtime name (e.g., "ffi", "python").
    pub fn runtime_name(&self) -> &'static str {
        match self {
            LogicRuntime::Python(_) => "python",
            LogicRuntime::Ffi(_) => "ffi",
            LogicRuntime::Java(_) => "java",
            LogicRuntime::Docker(_) => "docker",
            LogicRuntime::Ipc(_) => "ipc",
        }
    }

    /// Reconstruct the logic portion of the FROM string.
    pub fn to_logic_string(&self) -> String {
        self.inner().to_logic_string()
    }

    /// Return the file path if this runtime references a local file.
    pub fn file_path(&self) -> Option<&str> {
        self.inner().file_path()
    }

    /// Build the prefixed call string for IPC/native dispatch.
    pub fn build_call_string(&self) -> String {
        self.inner().build_call_string()
    }

    /// Validate that the referenced logic (file, module, etc.) is reachable.
    pub fn validate_logic(&self) -> Result<(), BundlebaseError> {
        self.inner().validate_logic()
    }

    /// Verify this runtime's bundled artifact is loadable (e.g., load manifest from shared lib).
    pub fn verify_loadable(&self) -> Result<(), BundlebaseError> {
        self.inner().verify_loadable()
    }

    /// Load the function manifest for wildcard discovery.
    pub fn load_manifest(&self) -> Result<Option<Manifest>, BundlebaseError> {
        self.inner().load_manifest()
    }

    /// Look up a single function's metadata from manifest.
    pub fn lookup_function_in_manifest(
        &self,
        function_name: &str,
    ) -> Result<ManifestEntry, BundlebaseError> {
        self.inner().lookup_function_in_manifest(function_name)
    }

    /// Invoke a scalar function.
    pub fn invoke_scalar(
        &self,
        name: &str,
        function_name: &str,
        args: &datafusion::logical_expr::ScalarFunctionArgs,
        subprocess_cache: &crate::function::ipc_bridge::SubprocessCache,
    ) -> datafusion::common::Result<datafusion::logical_expr::ColumnarValue> {
        self.inner().invoke_scalar(name, function_name, args, subprocess_cache)
    }

    /// Create an accumulator for an aggregate function.
    pub fn create_accumulator(
        &self,
        name: &str,
        function_name: &str,
        return_type: &arrow::datatypes::DataType,
        subprocess_cache: &crate::function::ipc_bridge::SubprocessCache,
    ) -> datafusion::common::Result<Box<dyn datafusion::logical_expr::Accumulator>> {
        self.inner().create_accumulator(name, function_name, return_type, subprocess_cache)
    }

    /// DataType for aggregate state serialization.
    pub fn aggregate_state_type(&self, return_type: &arrow::datatypes::DataType) -> arrow::datatypes::DataType {
        self.inner().aggregate_state_type(return_type)
    }

    /// Copy the file referenced by this LogicRuntime into the bundle's data directory.
    ///
    /// For runtimes that reference local files (Ffi, Ipc, Java), reads the file,
    /// writes it into the bundle's content-addressed storage via `write_stream()`,
    /// and returns a new LogicRuntime pointing to the bundle-relative location.
    ///
    /// For runtimes that don't reference files (Docker, Python), returns as-is.
    pub async fn copy_into_bundle(
        &self,
        data_dir: &Arc<dyn IOReadWriteDir>,
    ) -> Result<LogicRuntime, BundlebaseError> {
        match self.inner().copy_into_bundle(data_dir).await? {
            Some(new_path) => Ok(self.clone().with_path(new_path)),
            None => Ok(self.clone()),
        }
    }

    /// Resolve a logic path that may be bundle-relative against the bundle's data directory.
    ///
    /// If the runtime references a file and the path is relative (bundle-internal hash path),
    /// it's resolved against the data_dir's filesystem path.
    /// Absolute paths and non-file runtimes are returned as-is.
    pub fn resolve_path(&self, data_dir: &Arc<dyn IOReadWriteDir>) -> LogicRuntime {
        let file_path = match self.file_path() {
            Some(p) => p,
            None => return self.clone(),
        };

        // Only resolve relative paths (bundle-internal hash paths like "ab/cdef12345678.so")
        if file_path.starts_with('/') || file_path.starts_with("./") || file_path.starts_with("../") {
            return self.clone();
        }

        // Extract filesystem path from data_dir URL
        let url = data_dir.url();
        if url.scheme() != "file" {
            return self.clone();
        }

        let dir_path = url.path();
        let resolved = format!("{}/{}", dir_path, file_path);

        self.clone().with_path(resolved)
    }

    /// Verify that a bundled function binary is functional from its new location.
    ///
    /// After `copy_into_bundle()`, this loads the manifest from the bundled copy
    /// to confirm the file is readable and executable. Skipped for runtimes that don't
    /// reference files (Docker, Python).
    pub async fn verify_bundled_function(
        &self,
        data_dir: &Arc<dyn IOReadWriteDir>,
    ) -> Result<(), BundlebaseError> {
        if self.file_path().is_none() {
            return Ok(());
        }

        let resolved = self.resolve_path(data_dir);
        resolved.verify_loadable()
    }

    /// Verify that a bundled connector binary is functional from its new location.
    ///
    /// After `copy_into_bundle()`, spawns the connector from its bundled path,
    /// performs a JSON-RPC handshake, and shuts it down. This confirms the binary is
    /// executable and responds from its new location.
    pub async fn verify_bundled_connector(
        &self,
        data_dir: &Arc<dyn IOReadWriteDir>,
    ) -> Result<(), BundlebaseError> {
        if self.file_path().is_none() {
            return Ok(());
        }

        let resolved = self.resolve_path(data_dir);
        let call_string = resolved.build_call_string();

        verify_ipc_handshake(&call_string).await
    }
}

impl fmt::Display for LogicRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.runtime_name())
    }
}

/// Custom serialization: serialize as the FROM string.
impl Serialize for LogicRuntime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_from_string())
    }
}

/// Custom deserialization: deserialize from a FROM string.
impl<'de> Deserialize<'de> for LogicRuntime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        LogicRuntime::parse_from(&s).map_err(serde::de::Error::custom)
    }
}

/// Spawn an IPC subprocess, perform a handshake, then shut it down.
///
/// Used as a smoke test to verify a bundled connector binary is functional.
/// Accepts both success and `method_not_found` responses as valid — the point
/// is just to confirm the binary can be spawned and responds to JSON-RPC.
async fn verify_ipc_handshake(call_string: &str) -> Result<(), BundlebaseError> {
    let command = crate::function::ipc_bridge::parse_call(call_string)?;

    if command.is_empty() {
        return Err("Cannot verify connector with empty command".into());
    }

    let mut child = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| {
            BundlebaseError::from(format!(
                "Bundled connector verification failed: could not spawn '{}': {}",
                command[0], e
            ))
        })?;

    let stdin = child
        .stdin
        .take()
        .ok_or("Failed to capture stdin for connector verification")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Failed to capture stdout for connector verification")?;

    let mut writer = tokio::io::BufWriter::new(stdin);
    let mut reader = tokio::io::BufReader::new(stdout);

    // Send handshake request
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "handshake",
        "params": {"protocol_version": "1"}
    });
    let mut line = serde_json::to_string(&request)
        .map_err(|e| format!("Failed to serialize handshake request: {}", e))?;
    line.push('\n');

    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;

    writer
        .write_all(line.as_bytes())
        .await
        .map_err(|e| format!("Connector verification: failed to write handshake: {}", e))?;
    writer
        .flush()
        .await
        .map_err(|e| format!("Connector verification: failed to flush handshake: {}", e))?;

    // Read response (with timeout)
    let mut response_line = String::new();
    let read_result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        reader.read_line(&mut response_line),
    )
    .await;

    // Send shutdown and kill regardless of response
    let shutdown = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "shutdown",
        "params": {}
    });
    let mut shutdown_line = serde_json::to_string(&shutdown).unwrap_or_default();
    shutdown_line.push('\n');
    let _ = writer.write_all(shutdown_line.as_bytes()).await;
    let _ = writer.flush().await;
    let _ = child.kill().await;

    // Now check the read result
    match read_result {
        Ok(Ok(0)) => {
            Err("Connector verification failed: subprocess closed stdout without responding".into())
        }
        Ok(Ok(_)) => {
            // Got a response — parse it to check for errors (but method_not_found is OK)
            if let Ok(resp) = serde_json::from_str::<serde_json::Value>(response_line.trim()) {
                if let Some(err) = resp.get("error") {
                    let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                    if code != -32601 {
                        // Not method_not_found — report the error
                        let message = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        return Err(format!(
                            "Connector verification failed: handshake error (code {}): {}",
                            code, message
                        )
                        .into());
                    }
                }
            }
            Ok(())
        }
        Ok(Err(e)) => Err(format!(
            "Connector verification failed: error reading response: {}",
            e
        )
        .into()),
        Err(_) => Err(
            "Connector verification failed: subprocess did not respond within 10 seconds".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_from_ipc_relative() {
        let rt = LogicRuntime::parse_from("ipc::./my_func").unwrap();
        assert_eq!(rt.runtime_name(), "ipc");
        assert_eq!(rt.to_logic_string(), "./my_func");
    }

    #[test]
    fn test_parse_from_ipc_absolute() {
        let rt = LogicRuntime::parse_from("ipc::/usr/bin/func").unwrap();
        assert_eq!(rt.runtime_name(), "ipc");
        assert_eq!(rt.to_logic_string(), "/usr/bin/func");
    }

    #[test]
    fn test_parse_from_ffi() {
        let rt = LogicRuntime::parse_from("ffi::./mylib.so").unwrap();
        assert_eq!(rt.runtime_name(), "ffi");
        assert_eq!(rt.to_logic_string(), "./mylib.so");
    }

    #[test]
    fn test_parse_from_lib_compat() {
        let rt = LogicRuntime::parse_from("lib::./mylib.so").unwrap();
        assert_eq!(rt.runtime_name(), "ffi");
        assert_eq!(rt.to_logic_string(), "./mylib.so");
    }

    #[test]
    fn test_parse_from_python() {
        let rt = LogicRuntime::parse_from("python::mod:func").unwrap();
        assert_eq!(rt.runtime_name(), "python");
        assert_eq!(rt.to_logic_string(), "mod:func");
    }

    #[test]
    fn test_parse_from_docker() {
        let rt = LogicRuntime::parse_from("docker::my-image").unwrap();
        assert_eq!(rt.runtime_name(), "docker");
        assert_eq!(rt.to_logic_string(), "my-image");
    }

    #[test]
    fn test_parse_from_java() {
        let rt = LogicRuntime::parse_from("java::com.example.MyClass").unwrap();
        assert_eq!(rt.runtime_name(), "java");
        assert_eq!(rt.to_logic_string(), "com.example.MyClass");
    }

    #[test]
    fn test_parse_from_invalid_no_separator() {
        assert!(LogicRuntime::parse_from("ipc:./my_func").is_err());
    }

    #[test]
    fn test_parse_from_invalid_empty_logic() {
        assert!(LogicRuntime::parse_from("ipc::").is_err());
    }

    #[test]
    fn test_parse_from_invalid_runtime() {
        assert!(LogicRuntime::parse_from("unknown::./func").is_err());
    }

    #[test]
    fn test_to_from_string() {
        let rt = LogicRuntime::parse_from("ipc::./my_func").unwrap();
        assert_eq!(rt.to_from_string(), "ipc::./my_func");

        let rt = LogicRuntime::parse_from("ffi::./mylib.so").unwrap();
        assert_eq!(rt.to_from_string(), "ffi::./mylib.so");

        let rt = LogicRuntime::parse_from("python::mod:func").unwrap();
        assert_eq!(rt.to_from_string(), "python::mod:func");

        let rt = LogicRuntime::parse_from("ipc::/usr/bin/func").unwrap();
        assert_eq!(rt.to_from_string(), "ipc::/usr/bin/func");
    }

    #[test]
    fn test_from_roundtrip() {
        let from = "ipc::./my_func";
        let rt = LogicRuntime::parse_from(from).unwrap();
        assert_eq!(rt.to_from_string(), from);
    }

    #[test]
    fn test_find_in_manifest_not_found_lists_available() {
        let manifest = Manifest {
            functions: vec![
                ManifestEntry {
                    name: "add".to_string(),
                    symbol: None,
                    input_types: vec!["Int64".to_string()],
                    return_type: "Int64".to_string(),
                    kind: "scalar".to_string(),
                },
                ManifestEntry {
                    name: "multiply".to_string(),
                    symbol: None,
                    input_types: vec!["Int64".to_string()],
                    return_type: "Int64".to_string(),
                    kind: "scalar".to_string(),
                },
            ],
        };

        let err = find_in_manifest(manifest, "nonexistent", "ipc::./my_func")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("nonexistent"),
            "Error should mention the requested function: {}",
            err
        );
        assert!(
            err.contains("add") && err.contains("multiply"),
            "Error should list available functions: {}",
            err
        );
    }

    #[test]
    fn test_find_in_manifest_empty_manifest() {
        let manifest = Manifest {
            functions: vec![],
        };

        let err = find_in_manifest(manifest, "nonexistent", "ipc::./my_func")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no functions"),
            "Error should indicate empty manifest: {}",
            err
        );
    }

    #[test]
    fn test_logic_runtime_can_bundle() {
        assert!(!LogicRuntime::parse_from("python::mod:func").unwrap().can_bundle());
        assert!(LogicRuntime::parse_from("ffi::./lib.so").unwrap().can_bundle());
        assert!(LogicRuntime::parse_from("java::./my.jar").unwrap().can_bundle());
        assert!(LogicRuntime::parse_from("docker::my-image").unwrap().can_bundle());
        assert!(LogicRuntime::parse_from("ipc::./func").unwrap().can_bundle());
    }

    #[test]
    fn test_logic_runtime_serde_roundtrip() {
        let rt = LogicRuntime::parse_from("ffi::./mylib.so:double_val").unwrap();
        let yaml = serde_yaml_ng::to_string(&rt).unwrap();
        assert!(yaml.contains("ffi::./mylib.so:double_val"));
        let deser: LogicRuntime = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(deser, rt);
    }

    #[test]
    fn test_logic_runtime_serde_roundtrip_ipc() {
        let rt = LogicRuntime::parse_from("ipc::./my_func").unwrap();
        let yaml = serde_yaml_ng::to_string(&rt).unwrap();
        let deser: LogicRuntime = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(deser, rt);
    }

    #[test]
    fn test_logic_runtime_serde_roundtrip_python() {
        let rt = LogicRuntime::parse_from("python::mod:func").unwrap();
        let yaml = serde_yaml_ng::to_string(&rt).unwrap();
        let deser: LogicRuntime = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(deser, rt);
    }

    #[tokio::test]
    async fn test_copy_into_bundle_docker_noop() {
        let dir = crate::test_utils::random_memory_dir();
        let from = LogicRuntime::parse_from("docker::my-image:latest").unwrap();
        let result = from.copy_into_bundle(&dir)
            .await
            .unwrap();
        assert_eq!(result.to_logic_string(), "my-image:latest");
    }

    #[tokio::test]
    async fn test_copy_into_bundle_lib() {
        let dir = crate::test_utils::random_memory_dir();

        // Create a temp file to copy
        let tmp_dir = tempfile::tempdir().unwrap();
        let lib_path = tmp_dir.path().join("mylib.so");
        std::fs::write(&lib_path, b"fake library content").unwrap();

        let from_str = format!("ffi::{}", lib_path.to_str().unwrap());
        let from = LogicRuntime::parse_from(&from_str).unwrap();
        let result = from.copy_into_bundle(&dir)
            .await
            .unwrap();

        // Result should be a hash-based path with .so extension
        let logic = result.to_logic_string();
        assert!(logic.ends_with(".so"), "Expected .so extension, got: {}", logic);
        assert!(logic.contains('/'), "Expected hash dir separator, got: {}", logic);
        // Should be format: XX/YYYYYYYYYYYYYY.so
        let parts: Vec<&str> = logic.split('/').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 2); // 2-char hash prefix dir
    }

    #[tokio::test]
    async fn test_copy_into_bundle_lib_with_symbol() {
        let dir = crate::test_utils::random_memory_dir();

        let tmp_dir = tempfile::tempdir().unwrap();
        let lib_path = tmp_dir.path().join("mylib.so");
        std::fs::write(&lib_path, b"fake library content").unwrap();

        let from_str = format!("ffi::{}:double_val", lib_path.to_str().unwrap());
        let from = LogicRuntime::parse_from(&from_str).unwrap();
        let result = from.copy_into_bundle(&dir)
            .await
            .unwrap();

        // Result should preserve the symbol suffix
        let logic = result.to_logic_string();
        assert!(logic.ends_with(":double_val"), "Expected :double_val suffix, got: {}", logic);
        // Strip suffix and check path format
        let path_part = logic.strip_suffix(":double_val").unwrap();
        assert!(path_part.ends_with(".so"));
    }

    #[tokio::test]
    async fn test_copy_into_bundle_ipc() {
        let dir = crate::test_utils::random_memory_dir();

        let tmp_dir = tempfile::tempdir().unwrap();
        let exec_path = tmp_dir.path().join("my_func");
        std::fs::write(&exec_path, b"fake executable content").unwrap();

        let from_str = format!("ipc::{}", exec_path.to_str().unwrap());
        let from = LogicRuntime::parse_from(&from_str).unwrap();
        let result = from.copy_into_bundle(&dir)
            .await
            .unwrap();

        let logic = result.to_logic_string();
        assert!(logic.ends_with(".bin"), "Expected .bin extension for extensionless file, got: {}", logic);
    }

    #[tokio::test]
    async fn test_copy_into_bundle_java() {
        let dir = crate::test_utils::random_memory_dir();

        let tmp_dir = tempfile::tempdir().unwrap();
        let jar_path = tmp_dir.path().join("my_connector.jar");
        std::fs::write(&jar_path, b"fake jar content").unwrap();

        let from_str = format!("java::{}", jar_path.to_str().unwrap());
        let from = LogicRuntime::parse_from(&from_str).unwrap();
        let result = from.copy_into_bundle(&dir)
            .await
            .unwrap();

        let logic = result.to_logic_string();
        assert!(logic.ends_with(".jar"), "Expected .jar extension, got: {}", logic);
        assert!(logic.contains('/'), "Expected hash dir separator, got: {}", logic);
        let parts: Vec<&str> = logic.split('/').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 2); // 2-char hash prefix dir
    }

    #[tokio::test]
    async fn test_copy_into_bundle_java_with_classname() {
        let dir = crate::test_utils::random_memory_dir();

        let tmp_dir = tempfile::tempdir().unwrap();
        let jar_path = tmp_dir.path().join("my_connector.jar");
        std::fs::write(&jar_path, b"fake jar content").unwrap();

        let from_str = format!("java::{}:com.example.MyClass", jar_path.to_str().unwrap());
        let from = LogicRuntime::parse_from(&from_str).unwrap();
        let result = from.copy_into_bundle(&dir)
            .await
            .unwrap();

        let logic = result.to_logic_string();
        assert!(
            logic.ends_with(":com.example.MyClass"),
            "Expected :com.example.MyClass suffix, got: {}",
            logic
        );
        let path_part = logic.strip_suffix(":com.example.MyClass").unwrap();
        assert!(path_part.ends_with(".jar"));
    }

    #[test]
    fn test_validate_logic_ipc_nonexistent() {
        let rt = LogicRuntime::parse_from("ipc::./nonexistent_binary_xyz").unwrap();
        let err = rt.validate_logic().unwrap_err().to_string();
        assert!(err.contains("IPC executable"), "Expected 'IPC executable' in error: {}", err);
        assert!(err.contains("not found"), "Expected 'not found' in error: {}", err);
    }

    #[test]
    fn test_validate_logic_ffi_nonexistent() {
        let rt = LogicRuntime::parse_from("ffi::./nonexistent_lib_xyz.so").unwrap();
        let err = rt.validate_logic().unwrap_err().to_string();
        assert!(err.contains("Shared library"), "Expected 'Shared library' in error: {}", err);
        assert!(err.contains("not found"), "Expected 'not found' in error: {}", err);
    }

    #[test]
    fn test_validate_logic_docker_always_ok() {
        let rt = LogicRuntime::parse_from("docker::any-image:latest").unwrap();
        assert!(rt.validate_logic().is_ok());
    }

    #[test]
    fn test_validate_logic_java_nonexistent() {
        let rt = LogicRuntime::parse_from("java::./nonexistent_xyz.jar").unwrap();
        let err = rt.validate_logic().unwrap_err().to_string();
        assert!(err.contains("JAR file"), "Expected 'JAR file' in error: {}", err);
        assert!(err.contains("not found"), "Expected 'not found' in error: {}", err);
    }

    #[test]
    fn test_resolve_path_absolute() {
        let dir = crate::test_utils::random_memory_dir();
        let from = LogicRuntime::parse_from("ffi::/usr/lib/mylib.so:symbol").unwrap();
        let result = from.resolve_path(&dir);
        assert_eq!(result.to_logic_string(), "/usr/lib/mylib.so:symbol");
    }

    #[test]
    fn test_resolve_path_relative_dot() {
        let dir = crate::test_utils::random_memory_dir();
        let from = LogicRuntime::parse_from("ffi::./mylib.so").unwrap();
        let result = from.resolve_path(&dir);
        assert_eq!(result.to_logic_string(), "./mylib.so");
    }

    #[test]
    fn test_resolve_path_bundle_relative() {
        // Memory dirs have memory:// scheme, not file://, so resolution won't apply
        let dir = crate::test_utils::random_memory_dir();
        let from = LogicRuntime::parse_from("ffi::ab/cdef12345678.so").unwrap();
        let result = from.resolve_path(&dir);
        assert_eq!(result.to_logic_string(), "ab/cdef12345678.so");
    }
}
