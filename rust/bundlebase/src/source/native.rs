//! Built-in "native" source function.
//!
//! Loads user source functions in-process for zero-copy Arrow data transfer.
//! Two strategies based on the `call` argument:
//!
//! - `lib:/path/to/lib.so` — loads a shared library via `dlopen` and uses the
//!   Arrow C Data Interface (`ArrowArrayStream`) for zero-copy streaming.
//! - `python:module:Class` — delegates to a `NativePythonBridge` trait object
//!   registered by the Python bindings at init time (PyO3 + `FromPyArrow`).

use super::source_function::{
    ArgSpec, DiscoveredLocation, SourceData, SourceFunction, SourceFunctionSignature,
};
use super::source_utils;
use crate::bundle_config::is_external_code_allowed;
use crate::{BundleConfig, BundlebaseError};

use arrow::ffi_stream::{ArrowArrayStreamReader, FFI_ArrowArrayStream};
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::{Arc, OnceLock};
use url::Url;

// ---------------------------------------------------------------------------
// NativePythonBridge — trait for the PyO3 bridge
// ---------------------------------------------------------------------------

/// Trait that the Python bindings implement to provide in-process access
/// to Python `SourceFunction` objects.
pub trait NativePythonBridge: Send + Sync {
    /// Call `discover()` on the Python source, returning locations as JSON.
    fn discover(&self, call: &str, args_json: &str) -> Result<String, BundlebaseError>;

    /// Call `data()` on the Python source, returning record batches.
    fn data(
        &self,
        call: &str,
        location_json: &str,
        args_json: &str,
    ) -> Result<Option<Vec<RecordBatch>>, BundlebaseError>;

    /// Call `stable_url()` on the Python source.
    fn stable_url(
        &self,
        call: &str,
        location_json: &str,
        args_json: &str,
    ) -> Result<Option<String>, BundlebaseError>;
}

/// Global bridge set by `bundlebase-python` at module init time.
static PYTHON_BRIDGE: OnceLock<Arc<dyn NativePythonBridge>> = OnceLock::new();

/// Register the Python bridge. Called once from `bundlebase-python` init.
pub fn register_python_bridge(bridge: Arc<dyn NativePythonBridge>) {
    let _ = PYTHON_BRIDGE.set(bridge);
}

/// Get the registered Python bridge.
fn get_python_bridge() -> Result<&'static Arc<dyn NativePythonBridge>, BundlebaseError> {
    PYTHON_BRIDGE
        .get()
        .ok_or_else(|| "Python plugin bridge not initialized. Are you running from Python?".into())
}

// ---------------------------------------------------------------------------
// C ABI types for shared libraries
// ---------------------------------------------------------------------------

type DiscoverFn =
    unsafe extern "C" fn(args_json: *const c_char, out_json: *mut *mut c_char) -> i32;
type DataFn = unsafe extern "C" fn(
    location_json: *const c_char,
    args_json: *const c_char,
    out: *mut FFI_ArrowArrayStream,
) -> i32;
type FreeFn = unsafe extern "C" fn(ptr: *mut c_char);
type StableUrlFn =
    unsafe extern "C" fn(location_json: *const c_char, args_json: *const c_char, out_json: *mut *mut c_char) -> i32;

// ---------------------------------------------------------------------------
// SharedLibStrategy
// ---------------------------------------------------------------------------

struct SharedLibHandle {
    _lib: libloading::Library,
    discover: DiscoverFn,
    data: DataFn,
    free: FreeFn,
    stable_url: Option<StableUrlFn>,
}

// SAFETY: The shared library functions are expected to be thread-safe by contract.
// The C ABI requires the library author to ensure thread safety.
unsafe impl Send for SharedLibHandle {}
unsafe impl Sync for SharedLibHandle {}

impl SharedLibHandle {
    fn load(path: &str) -> Result<Self, BundlebaseError> {
        // SAFETY: We trust the user-provided shared library to export the correct C ABI.
        // The library is loaded once and kept alive for the lifetime of the source function.
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| format!("Failed to load shared library '{}': {}", path, e))?;

        let discover: DiscoverFn = unsafe {
            *lib.get(b"bundlebase_discover\0")
                .map_err(|e| format!("Symbol 'bundlebase_discover' not found in '{}': {}", path, e))?
        };
        let data: DataFn = unsafe {
            *lib.get(b"bundlebase_data\0")
                .map_err(|e| format!("Symbol 'bundlebase_data' not found in '{}': {}", path, e))?
        };
        let free: FreeFn = unsafe {
            *lib.get(b"bundlebase_free\0")
                .map_err(|e| format!("Symbol 'bundlebase_free' not found in '{}': {}", path, e))?
        };
        let stable_url: Option<StableUrlFn> = unsafe {
            lib.get(b"bundlebase_stable_url\0").ok().map(|s| *s)
        };

        Ok(Self {
            _lib: lib,
            discover,
            data,
            free,
            stable_url,
        })
    }

    fn call_discover(&self, args_json: &str) -> Result<String, BundlebaseError> {
        let c_args = CString::new(args_json)
            .map_err(|_| BundlebaseError::from("args_json contains null byte"))?;
        let mut out_ptr: *mut c_char = std::ptr::null_mut();

        let rc = unsafe { (self.discover)(c_args.as_ptr(), &mut out_ptr) };
        if rc != 0 {
            // If out_ptr was set, it may contain an error message
            if !out_ptr.is_null() {
                let msg = unsafe { CStr::from_ptr(out_ptr) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { (self.free)(out_ptr) };
                return Err(format!("bundlebase_discover failed (code {}): {}", rc, msg).into());
            }
            return Err(format!("bundlebase_discover failed with code {}", rc).into());
        }

        if out_ptr.is_null() {
            return Err("bundlebase_discover returned null".into());
        }

        let json = unsafe { CStr::from_ptr(out_ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.free)(out_ptr) };

        Ok(json)
    }

    fn call_data(
        &self,
        location_json: &str,
        args_json: &str,
    ) -> Result<Option<Vec<RecordBatch>>, BundlebaseError> {
        let c_location = CString::new(location_json)
            .map_err(|_| BundlebaseError::from("location_json contains null byte"))?;
        let c_args = CString::new(args_json)
            .map_err(|_| BundlebaseError::from("args_json contains null byte"))?;

        let mut ffi_stream = FFI_ArrowArrayStream::empty();

        let rc = unsafe { (self.data)(c_location.as_ptr(), c_args.as_ptr(), &mut ffi_stream) };
        if rc != 0 {
            return Err(format!("bundlebase_data failed with code {}", rc).into());
        }

        // Check if stream was populated (release callback is non-null)
        if ffi_stream.release.is_none() {
            return Ok(None);
        }

        let reader = ArrowArrayStreamReader::try_new(ffi_stream)
            .map_err(|e| format!("Failed to create ArrowArrayStreamReader: {}", e))?;

        let batches: Result<Vec<RecordBatch>, _> = reader.collect();
        let batches =
            batches.map_err(|e| format!("Failed to read record batches from stream: {}", e))?;

        if batches.is_empty() {
            return Ok(None);
        }

        Ok(Some(batches))
    }

    fn call_stable_url(
        &self,
        location_json: &str,
        args_json: &str,
    ) -> Result<Option<String>, BundlebaseError> {
        let func = match self.stable_url {
            Some(f) => f,
            None => return Ok(None),
        };

        let c_location = CString::new(location_json)
            .map_err(|_| BundlebaseError::from("location_json contains null byte"))?;
        let c_args = CString::new(args_json)
            .map_err(|_| BundlebaseError::from("args_json contains null byte"))?;
        let mut out_ptr: *mut c_char = std::ptr::null_mut();

        let rc = unsafe { (func)(c_location.as_ptr(), c_args.as_ptr(), &mut out_ptr) };
        if rc != 0 {
            if !out_ptr.is_null() {
                let msg = unsafe { CStr::from_ptr(out_ptr) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { (self.free)(out_ptr) };
                return Err(format!("bundlebase_stable_url failed (code {}): {}", rc, msg).into());
            }
            return Err(format!("bundlebase_stable_url failed with code {}", rc).into());
        }

        if out_ptr.is_null() {
            return Ok(None);
        }

        let json = unsafe { CStr::from_ptr(out_ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.free)(out_ptr) };

        if json.is_empty() || json == "null" {
            return Ok(None);
        }

        let parsed: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| format!("Failed to parse stable_url response: {}", e))?;
        Ok(parsed.get("url").and_then(|v| v.as_str()).map(String::from))
    }
}

// ---------------------------------------------------------------------------
// NativeSourceFunction
// ---------------------------------------------------------------------------

/// Built-in "native" source function for in-process data loading.
///
/// Supports two call strategies:
/// - `lib:/path/to/lib.so` — shared library via Arrow C Data Interface
/// - `python:module:Class` — Python in-process via PyO3
pub struct NativeSourceFunction {
    lib_handle: tokio::sync::Mutex<Option<SharedLibHandle>>,
}

impl NativeSourceFunction {
    pub fn new() -> Self {
        Self {
            lib_handle: tokio::sync::Mutex::new(None),
        }
    }

    /// Ensure the shared library is loaded (only for `lib:` calls).
    async fn ensure_lib_loaded(
        &self,
        path: &str,
    ) -> Result<(), BundlebaseError> {
        let mut guard = self.lib_handle.lock().await;
        if guard.is_none() {
            let handle = SharedLibHandle::load(path)?;
            *guard = Some(handle);
        }
        Ok(())
    }
}

/// Build a JSON string of args for the C ABI, excluding `call` and `copy`.
fn filtered_args_json(args: &HashMap<String, String>) -> Result<String, BundlebaseError> {
    let filtered: HashMap<&str, &str> = args
        .iter()
        .filter(|(k, _)| k.as_str() != "call" && k.as_str() != "copy")
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    serde_json::to_string(&filtered)
        .map_err(|e| format!("Failed to serialize args: {}", e).into())
}

/// Build a JSON string for a DiscoveredLocation.
fn location_json(location: &DiscoveredLocation) -> Result<String, BundlebaseError> {
    serde_json::to_string(&serde_json::json!({
        "location": location.location,
        "must_copy": location.must_copy,
        "format": location.format,
        "version": location.version,
    }))
    .map_err(|e| format!("Failed to serialize location: {}", e).into())
}

/// Parse the call argument to determine the strategy.
enum CallStrategy {
    SharedLib(String),
    Python(String),
}

fn parse_native_call(call: &str) -> Result<CallStrategy, BundlebaseError> {
    let call = call.trim();
    if let Some(path) = call.strip_prefix("lib:") {
        let path = path.trim();
        if path.is_empty() {
            return Err("lib: call requires a library path".into());
        }
        Ok(CallStrategy::SharedLib(path.to_string()))
    } else if let Some(rest) = call.strip_prefix("python:") {
        let rest = rest.trim();
        if rest.is_empty() {
            return Err("python: call requires module:Class format".into());
        }
        Ok(CallStrategy::Python(call.to_string()))
    } else {
        Err(format!(
            "Native source 'call' must start with 'lib:' or 'python:'. Got: '{}'",
            call
        )
        .into())
    }
}

/// Parse discover JSON response into DiscoveredLocations.
fn parse_discover_json(json: &str) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("Failed to parse discover response: {}", e))?;

    let locations = value
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

/// Build the discover args JSON including attached_locations.
fn build_discover_args_json(
    args: &HashMap<String, String>,
    attached_locations: &HashSet<String>,
) -> Result<String, BundlebaseError> {
    let filtered: HashMap<&str, &str> = args
        .iter()
        .filter(|(k, _)| k.as_str() != "call" && k.as_str() != "copy")
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut value = serde_json::to_value(&filtered)
        .map_err(|e| format!("Failed to serialize args: {}", e))?;
    value["attached_locations"] = serde_json::to_value(attached_locations)
        .map_err(|e| format!("Failed to serialize attached_locations: {}", e))?;
    serde_json::to_string(&value)
        .map_err(|e| format!("Failed to serialize discover args: {}", e).into())
}

#[async_trait]
impl SourceFunction for NativeSourceFunction {
    fn signature(&self) -> SourceFunctionSignature {
        SourceFunctionSignature {
            name: "native".to_string(),
            arg_specs: vec![
                ArgSpec {
                    name: "call",
                    description: "Source to load: 'lib:/path/to/lib.so' for shared library or 'python:module:Class' for Python in-process",
                    required: true,
                    default: None,
                },
                ArgSpec {
                    name: "copy",
                    description: "Whether to copy data into the bundle (default: true)",
                    required: false,
                    default: Some("true"),
                },
            ],
            accepts_extra_args: true,
        }
    }

    fn validate_args(&self, args: &HashMap<String, String>) -> Result<(), BundlebaseError> {
        let call = source_utils::require_arg(args, "call", "native")?;
        parse_native_call(call)?;
        Ok(())
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        if !is_external_code_allowed(_config)? {
            return Err("External code execution is disabled. Set system.allow_external_code=true to enable native sources.".into());
        }
        let call = source_utils::require_arg(args, "call", "native")?;

        match parse_native_call(call)? {
            CallStrategy::SharedLib(path) => {
                self.ensure_lib_loaded(&path).await?;
                let guard = self.lib_handle.lock().await;
                let handle = guard.as_ref().ok_or("Shared library not loaded")?;
                let args_json = build_discover_args_json(args, attached_locations)?;
                let response = handle.call_discover(&args_json)?;
                parse_discover_json(&response)
            }
            CallStrategy::Python(call_str) => {
                let bridge = get_python_bridge()?;
                let args_json = build_discover_args_json(args, attached_locations)?;
                let response = bridge.discover(&call_str, &args_json)?;
                parse_discover_json(&response)
            }
        }
    }

    async fn data(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Option<SourceData>, BundlebaseError> {
        if !is_external_code_allowed(_config)? {
            return Err("External code execution is disabled. Set system.allow_external_code=true to enable native sources.".into());
        }
        let call = source_utils::require_arg(args, "call", "native")?;
        let loc_json = location_json(location)?;
        let args_json = filtered_args_json(args)?;

        match parse_native_call(call)? {
            CallStrategy::SharedLib(_) => {
                let guard = self.lib_handle.lock().await;
                let handle = guard.as_ref().ok_or("Shared library not loaded")?;
                match handle.call_data(&loc_json, &args_json)? {
                    Some(batches) => {
                        let batch_stream = Box::pin(futures::stream::iter(
                            batches.into_iter().map(Ok),
                        ));
                        Ok(Some(SourceData::Arrow(batch_stream)))
                    }
                    None => Ok(None),
                }
            }
            CallStrategy::Python(call_str) => {
                let bridge = get_python_bridge()?;
                match bridge.data(&call_str, &loc_json, &args_json)? {
                    Some(batches) => {
                        let batch_stream = Box::pin(futures::stream::iter(
                            batches.into_iter().map(Ok),
                        ));
                        Ok(Some(SourceData::Arrow(batch_stream)))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    async fn stable_url(
        &self,
        location: &DiscoveredLocation,
        args: &HashMap<String, String>,
        _config: &Arc<BundleConfig>,
    ) -> Result<Option<Url>, BundlebaseError> {
        if !is_external_code_allowed(_config)? {
            return Err("External code execution is disabled. Set system.allow_external_code=true to enable native sources.".into());
        }
        let call = source_utils::require_arg(args, "call", "native")?;
        let loc_json = location_json(location)?;
        let args_json = filtered_args_json(args)?;

        let url_str = match parse_native_call(call)? {
            CallStrategy::SharedLib(_) => {
                let guard = self.lib_handle.lock().await;
                let handle = guard.as_ref().ok_or("Shared library not loaded")?;
                handle.call_stable_url(&loc_json, &args_json)?
            }
            CallStrategy::Python(call_str) => {
                let bridge = get_python_bridge()?;
                bridge.stable_url(&call_str, &loc_json, &args_json)?
            }
        };

        match url_str {
            Some(s) => {
                let url = Url::parse(&s)
                    .map_err(|e| format!("Invalid stable URL '{}': {}", s, e))?;
                Ok(Some(url))
            }
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle_config::{PassedBundleConfig, Scope};

    #[test]
    fn test_parse_native_call_lib() {
        match parse_native_call("lib:/path/to/lib.so") {
            Ok(CallStrategy::SharedLib(path)) => assert_eq!(path, "/path/to/lib.so"),
            _ => panic!("Expected SharedLib"),
        }
    }

    #[test]
    fn test_parse_native_call_python() {
        match parse_native_call("python:my_module:MyClass") {
            Ok(CallStrategy::Python(call)) => assert_eq!(call, "python:my_module:MyClass"),
            _ => panic!("Expected Python"),
        }
    }

    #[test]
    fn test_parse_native_call_lib_empty() {
        assert!(parse_native_call("lib:").is_err());
    }

    #[test]
    fn test_parse_native_call_python_empty() {
        assert!(parse_native_call("python:").is_err());
    }

    #[test]
    fn test_parse_native_call_invalid() {
        assert!(parse_native_call("some_command").is_err());
    }

    #[test]
    fn test_parse_native_call_empty() {
        assert!(parse_native_call("").is_err());
    }

    #[test]
    fn test_native_signature() {
        let func = NativeSourceFunction::new();
        let sig = func.signature();
        assert_eq!(sig.name, "native");
        assert_eq!(sig.arg_specs.len(), 2);
        assert_eq!(sig.arg_specs[0].name, "call");
        assert!(sig.arg_specs[0].required);
        assert_eq!(sig.arg_specs[1].name, "copy");
        assert!(!sig.arg_specs[1].required);
    }

    #[test]
    fn test_parse_discover_json() {
        let json = r#"{"locations": [
            {"location": "file1.parquet", "must_copy": true, "format": "parquet", "version": "v1"},
            {"location": "file2.csv", "must_copy": false, "format": "csv"}
        ]}"#;
        let locations = parse_discover_json(json).expect("should parse");
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].location, "file1.parquet");
        assert!(locations[0].must_copy);
        assert_eq!(locations[0].format, "parquet");
        assert_eq!(locations[0].version, "v1");
        assert_eq!(locations[1].location, "file2.csv");
        assert!(!locations[1].must_copy);
        assert_eq!(locations[1].format, "csv");
        assert_eq!(locations[1].version, "");
    }

    #[test]
    fn test_filtered_args_json() {
        let mut args = HashMap::new();
        args.insert("call".to_string(), "lib:test.so".to_string());
        args.insert("copy".to_string(), "true".to_string());
        args.insert("custom".to_string(), "value".to_string());

        let json = filtered_args_json(&args).expect("should serialize");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("should parse JSON");
        assert!(parsed.get("call").is_none());
        assert!(parsed.get("copy").is_none());
        assert_eq!(parsed.get("custom").and_then(|v| v.as_str()), Some("value"));
    }

    #[test]
    fn test_location_json() {
        let loc = DiscoveredLocation {
            location: "test.parquet".to_string(),
            must_copy: true,
            format: "parquet".to_string(),
            version: "v1".to_string(),
        };
        let json = location_json(&loc).expect("should serialize");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("should parse JSON");
        assert_eq!(
            parsed.get("location").and_then(|v| v.as_str()),
            Some("test.parquet")
        );
    }

    #[test]
    fn test_validate_args_valid_lib() {
        let func = NativeSourceFunction::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "lib:/path/to/lib.so".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_valid_python() {
        let func = NativeSourceFunction::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "python:mod:Class".to_string());
        assert!(func.validate_args(&args).is_ok());
    }

    #[test]
    fn test_validate_args_invalid_call() {
        let func = NativeSourceFunction::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "invalid_call".to_string());
        assert!(func.validate_args(&args).is_err());
    }

    #[tokio::test]
    async fn test_discover_blocked_when_external_code_disabled() {
        let func = NativeSourceFunction::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "python:mod:Class".to_string());
        let config = Arc::new(BundleConfig::new(None).expect("test config creation"));

        let result = func.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("External code execution is disabled"));
    }

    #[tokio::test]
    async fn test_data_blocked_when_external_code_disabled() {
        let func = NativeSourceFunction::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "python:mod:Class".to_string());
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

    #[tokio::test]
    async fn test_stable_url_blocked_when_external_code_disabled() {
        let func = NativeSourceFunction::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "python:mod:Class".to_string());
        let config = Arc::new(BundleConfig::new(None).expect("test config creation"));
        let location = DiscoveredLocation {
            location: "test.parquet".to_string(),
            must_copy: true,
            format: "parquet".to_string(),
            version: "v1".to_string(),
        };

        let result = func.stable_url(&location, &args, &config).await;
        let err = result.err().expect("should fail");
        assert!(err.to_string().contains("External code execution is disabled"));
    }

    #[tokio::test]
    async fn test_discover_allowed_with_config() {
        // With allow_external_code=true, discover should pass the config gate
        // (will still fail because the Python bridge isn't initialized, but the
        // error should NOT be about external code being disabled)
        let func = NativeSourceFunction::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "python:mod:Class".to_string());

        let mut passed = PassedBundleConfig::new();
        passed.set(&Scope::try_from("system").expect("valid scope"), "allow_external_code", "true");
        let config = Arc::new(BundleConfig::new(Some(&passed)).expect("test config creation"));

        let result = func.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.expect_err("should fail (no bridge)");
        assert!(!err.to_string().contains("External code execution is disabled"));
    }
}
