//! Built-in "ffi" connector.
//!
//! Loads user connectors in-process for zero-copy Arrow data transfer.
//! Two strategies based on the `call` argument:
//!
//! - `ffi:/path/to/lib.so` — loads a shared library via `dlopen` and uses the
//!   Arrow C Data Interface (`ArrowArrayStream`) for zero-copy streaming.
//! - `python:module:Class` — delegates to a `NativePythonBridge` trait object
//!   registered by the Python bindings at init time (PyO3 + `FromPyArrow`).

use bundlebase_common::connector::{
    Connector, ConnectorSignature, DiscoveredLocation, SourceData, SourceFormat,
};
use bundlebase_common::source_utils as shared_utils;
use bundlebase_common::system_config::is_external_code_allowed;
use bundlebase_common::{BundlebaseError, ConfigProvider};

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
/// to Python `Connector` objects.
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

type DiscoverFn = unsafe extern "C" fn(args_json: *const c_char, out_json: *mut *mut c_char) -> i32;
type DataFn = unsafe extern "C" fn(
    location_json: *const c_char,
    args_json: *const c_char,
    out: *mut FFI_ArrowArrayStream,
) -> i32;
type FreeFn = unsafe extern "C" fn(ptr: *mut c_char);
type StableUrlFn = unsafe extern "C" fn(
    location_json: *const c_char,
    args_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32;

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
        // The library is loaded once and kept alive for the lifetime of the connector.
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| format!("Failed to load shared library '{}': {}", path, e))?;

        let discover: DiscoverFn = unsafe {
            *lib.get(b"bundlebase_discover\0").map_err(|e| {
                format!(
                    "Symbol 'bundlebase_discover' not found in '{}': {}",
                    path, e
                )
            })?
        };
        let data: DataFn = unsafe {
            *lib.get(b"bundlebase_data\0")
                .map_err(|e| format!("Symbol 'bundlebase_data' not found in '{}': {}", path, e))?
        };
        let free: FreeFn = unsafe {
            *lib.get(b"bundlebase_free\0")
                .map_err(|e| format!("Symbol 'bundlebase_free' not found in '{}': {}", path, e))?
        };
        let stable_url: Option<StableUrlFn> =
            unsafe { lib.get(b"bundlebase_stable_url\0").ok().map(|s| *s) };

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
    ) -> Result<Option<ArrowArrayStreamReader>, BundlebaseError> {
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

        Ok(Some(reader))
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

pub fn verify_shared_lib_connector(path: &str) -> Result<(), BundlebaseError> {
    SharedLibHandle::load(path).map(|_| ())
}

/// Confirm `path` looks like a shared-library file for `(target_os, target_arch)`
/// without `dlopen`-ing it.
///
/// Used for non-host platforms during multi-platform IMPORT CONNECTOR — the
/// build host can't load a foreign binary, but it can still check the file
/// header so a typo or wrong-platform mistake is caught at registration time
/// rather than at fetch time on the consumer.
///
/// `target_arch == "*"` skips the arch-byte check (useful for `linux/*`-style
/// patterns). `target_os == "*"` rejects: a wildcard OS has no defined header
/// format to validate against.
pub fn verify_shared_lib_header(
    path: &str,
    target_os: &str,
    target_arch: &str,
) -> Result<(), BundlebaseError> {
    if target_os == "*" {
        return Err(format!(
            "Cannot structurally verify '{}' against wildcard os '*' — pin the os in the platform string.",
            path
        )
        .into());
    }
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Failed to read shared library '{}': {}", path, e))?;

    match target_os {
        "linux" => verify_elf(&bytes, path, target_arch),
        "darwin" => verify_macho(&bytes, path, target_arch),
        "windows" => verify_pe(&bytes, path, target_arch),
        other => Err(format!(
            "Unsupported target os '{}' for structural verification (expected linux, darwin, or windows).",
            other
        )
        .into()),
    }
}

fn verify_elf(bytes: &[u8], path: &str, target_arch: &str) -> Result<(), BundlebaseError> {
    if bytes.len() < 20 || &bytes[0..4] != b"\x7FELF" {
        return Err(format!(
            "'{}' is not an ELF shared library (expected for linux/*).",
            path
        )
        .into());
    }
    if target_arch == "*" {
        return Ok(());
    }
    // e_machine is a u16 at offset 18, little-endian for ELF (which is what
    // we care about for amd64/arm64 — both are LE).
    let e_machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    let expected = match target_arch {
        "amd64" => 0x3E,
        "arm64" => 0xB7,
        "386" | "i386" => 0x03,
        other => {
            return Err(format!(
                "Unsupported linux arch '{}' for structural verification.",
                other
            )
            .into())
        }
    };
    if e_machine != expected {
        return Err(format!(
            "ELF '{}' e_machine 0x{:X} does not match linux/{} (expected 0x{:X}).",
            path, e_machine, target_arch, expected
        )
        .into());
    }
    Ok(())
}

fn verify_macho(bytes: &[u8], path: &str, target_arch: &str) -> Result<(), BundlebaseError> {
    if bytes.len() < 8 {
        return Err(format!("'{}' is too small to be a Mach-O binary.", path).into());
    }
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    // Fat (universal) binary — big-endian magic. Don't try to validate the
    // embedded slice list; assume the toolchain produced it correctly.
    const FAT_MAGIC_BE: u32 = 0xCAFEBABE;
    const FAT_MAGIC_BE_64: u32 = 0xCAFEBABF;
    if magic.swap_bytes() == FAT_MAGIC_BE || magic.swap_bytes() == FAT_MAGIC_BE_64 {
        return Ok(());
    }
    // Thin Mach-O — little-endian magic.
    const MH_MAGIC_64: u32 = 0xFEEDFACF;
    const MH_MAGIC_32: u32 = 0xFEEDFACE;
    if magic != MH_MAGIC_64 && magic != MH_MAGIC_32 {
        return Err(format!(
            "'{}' is not a Mach-O binary (expected for darwin/*).",
            path
        )
        .into());
    }
    if target_arch == "*" {
        return Ok(());
    }
    // cputype is at offset 4, little-endian.
    let cputype = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    // Strip the 0x01000000 ABI64 bit before comparing.
    let base_cputype = cputype & 0x00FFFFFF;
    let expected = match target_arch {
        "amd64" => 7u32,    // CPU_TYPE_X86_64 = CPU_TYPE_X86 (7) | ABI64
        "arm64" => 12u32,   // CPU_TYPE_ARM64 = CPU_TYPE_ARM (12) | ABI64
        other => {
            return Err(format!(
                "Unsupported darwin arch '{}' for structural verification.",
                other
            )
            .into())
        }
    };
    if base_cputype != expected {
        return Err(format!(
            "Mach-O '{}' cputype 0x{:X} does not match darwin/{} (expected base 0x{:X}).",
            path, cputype, target_arch, expected
        )
        .into());
    }
    Ok(())
}

fn verify_pe(bytes: &[u8], path: &str, target_arch: &str) -> Result<(), BundlebaseError> {
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return Err(format!(
            "'{}' is not a PE/DLL binary (expected for windows/*).",
            path
        )
        .into());
    }
    // PE header offset is a u32 at 0x3C.
    let pe_off = u32::from_le_bytes([bytes[0x3C], bytes[0x3D], bytes[0x3E], bytes[0x3F]]) as usize;
    if pe_off + 6 > bytes.len() || &bytes[pe_off..pe_off + 4] != b"PE\0\0" {
        return Err(format!("'{}' has no valid PE signature.", path).into());
    }
    if target_arch == "*" {
        return Ok(());
    }
    // COFF header machine field is u16 at pe_off+4.
    let machine = u16::from_le_bytes([bytes[pe_off + 4], bytes[pe_off + 5]]);
    let expected = match target_arch {
        "amd64" => 0x8664,
        "arm64" => 0xAA64,
        "386" | "i386" => 0x014C,
        other => {
            return Err(format!(
                "Unsupported windows arch '{}' for structural verification.",
                other
            )
            .into())
        }
    };
    if machine != expected {
        return Err(format!(
            "PE '{}' machine 0x{:X} does not match windows/{} (expected 0x{:X}).",
            path, machine, target_arch, expected
        )
        .into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// FfiConnector
// ---------------------------------------------------------------------------

/// Built-in "ffi" connector for in-process data loading.
///
/// Supports two call strategies:
/// - `ffi:/path/to/lib.so` — shared library via Arrow C Data Interface
/// - `python:module:Class` — Python in-process via PyO3
pub struct FfiConnector {
    lib_handle: tokio::sync::Mutex<Option<SharedLibHandle>>,
}

impl FfiConnector {
    pub fn new() -> Self {
        Self {
            lib_handle: tokio::sync::Mutex::new(None),
        }
    }

    /// Ensure the shared library is loaded (only for `ffi:` calls).
    async fn ensure_lib_loaded(&self, path: &str) -> Result<(), BundlebaseError> {
        let mut guard = self.lib_handle.lock().await;
        if guard.is_none() {
            let handle = SharedLibHandle::load(path)?;
            *guard = Some(handle);
        }
        Ok(())
    }
}

/// Build a JSON string of args for the C ABI, excluding `call`.
fn filtered_args_json(args: &HashMap<String, String>) -> Result<String, BundlebaseError> {
    let filtered: HashMap<&str, &str> = args
        .iter()
        .filter(|(k, _)| k.as_str() != "call")
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    serde_json::to_string(&filtered).map_err(|e| format!("Failed to serialize args: {}", e).into())
}

/// Build a JSON string for a DiscoveredLocation.
fn location_json(location: &DiscoveredLocation) -> Result<String, BundlebaseError> {
    serde_json::to_string(&serde_json::json!({
        "location": location.location,
        "must_copy": location.must_copy,
        "format": location.format.extension(),
        "version": location.version,
        "num_rows": location.num_rows,
    }))
    .map_err(|e| format!("Failed to serialize location: {}", e).into())
}

/// Parse the call argument to determine the strategy.
enum CallStrategy {
    SharedLib(String),
    Python(String),
}

fn parse_ffi_call(call: &str) -> Result<CallStrategy, BundlebaseError> {
    let call = call.trim();
    if let Some(path) = call.strip_prefix("ffi:") {
        let path = path.trim();
        if path.is_empty() {
            return Err("ffi: call requires a library path".into());
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
            "FFI connector 'call' must start with 'ffi:' or 'python:'. Got: '{}'",
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
        let format = SourceFormat::from_extension(
            loc.get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("parquet"),
        );
        let version = loc
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // `num_rows` must be present (either as a non-negative integer or
        // explicit JSON null). Missing it is a connector bug — leaving it
        // implicit would silently understate dry-run row deltas.
        let num_rows = match loc.get("num_rows") {
            Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Number(n)) => Some(n.as_u64().ok_or_else(|| {
                BundlebaseError::from(format!(
                    "Connector returned non-integer num_rows for location '{}': {}",
                    location, n
                ))
            })?),
            Some(other) => {
                return Err(format!(
                    "Connector returned non-numeric num_rows for location '{}': {:?}",
                    location, other
                )
                .into());
            }
            None => {
                return Err(format!(
                    "Connector did not return num_rows for location '{}' — \
                    return an integer when known cheaply, or explicit null \
                    when unknown",
                    location
                )
                .into());
            }
        };

        discovered.push(DiscoveredLocation {
            location,
            must_copy,
            format,
            version,
            num_rows,
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
        .filter(|(k, _)| k.as_str() != "call")
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let mut value =
        serde_json::to_value(&filtered).map_err(|e| format!("Failed to serialize args: {}", e))?;
    value["attached_locations"] = serde_json::to_value(attached_locations)
        .map_err(|e| format!("Failed to serialize attached_locations: {}", e))?;
    serde_json::to_string(&value)
        .map_err(|e| format!("Failed to serialize discover args: {}", e).into())
}

#[async_trait]
impl Connector for FfiConnector {
    fn signature(&self) -> ConnectorSignature {
        ConnectorSignature {
            name: "ffi".to_string(),
            arg_specs: vec![],
            // call is injected by source definition resolution; user kwargs pass through
            accepts_extra_args: true,
        }
    }

    async fn discover(
        &self,
        args: &HashMap<String, String>,
        attached_locations: &HashSet<String>,
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Vec<DiscoveredLocation>, BundlebaseError> {
        if !is_external_code_allowed(_config.as_ref())? {
            return Err("External code execution is disabled. Set system.allow_external_code=true to enable FFI sources.".into());
        }
        let call = shared_utils::require_arg(args, "call", "ffi")?;

        match parse_ffi_call(call)? {
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
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<SourceData>, BundlebaseError> {
        if !is_external_code_allowed(_config.as_ref())? {
            return Err("External code execution is disabled. Set system.allow_external_code=true to enable FFI sources.".into());
        }
        let call = shared_utils::require_arg(args, "call", "ffi")?;
        let loc_json = location_json(location)?;
        let args_json = filtered_args_json(args)?;

        match parse_ffi_call(call)? {
            CallStrategy::SharedLib(_) => {
                let guard = self.lib_handle.lock().await;
                let handle = guard.as_ref().ok_or("Shared library not loaded")?;
                match handle.call_data(&loc_json, &args_json)? {
                    Some(reader) => {
                        // Stream batches lazily from the ArrowArrayStreamReader
                        // instead of collecting all into memory.
                        let batch_stream =
                            Box::pin(futures::stream::unfold(reader, |mut reader| async move {
                                match reader.next() {
                                    Some(Ok(batch)) => Some((Ok(batch), reader)),
                                    Some(Err(e)) => Some((
                                        Err(BundlebaseError::from(format!(
                                            "Failed to read record batch from stream: {}",
                                            e
                                        ))),
                                        reader,
                                    )),
                                    None => None,
                                }
                            }));
                        Ok(Some(SourceData::Arrow(batch_stream)))
                    }
                    None => Ok(None),
                }
            }
            CallStrategy::Python(call_str) => {
                let bridge = get_python_bridge()?;
                match bridge.data(&call_str, &loc_json, &args_json)? {
                    Some(batches) => {
                        let batch_stream =
                            Box::pin(futures::stream::iter(batches.into_iter().map(Ok)));
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
        _config: &Arc<dyn ConfigProvider>,
    ) -> Result<Option<Url>, BundlebaseError> {
        if !is_external_code_allowed(_config.as_ref())? {
            return Err("External code execution is disabled. Set system.allow_external_code=true to enable FFI sources.".into());
        }
        let call = shared_utils::require_arg(args, "call", "ffi")?;
        let loc_json = location_json(location)?;
        let args_json = filtered_args_json(args)?;

        let url_str = match parse_ffi_call(call)? {
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
                let url =
                    Url::parse(&s).map_err(|e| format!("Invalid stable URL '{}': {}", s, e))?;
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

    #[test]
    fn test_parse_ffi_call_lib() {
        match parse_ffi_call("ffi:/path/to/lib.so") {
            Ok(CallStrategy::SharedLib(path)) => assert_eq!(path, "/path/to/lib.so"),
            _ => panic!("Expected SharedLib"),
        }
    }

    #[test]
    fn test_parse_ffi_call_python() {
        match parse_ffi_call("python:my_module:MyClass") {
            Ok(CallStrategy::Python(call)) => assert_eq!(call, "python:my_module:MyClass"),
            _ => panic!("Expected Python"),
        }
    }

    #[test]
    fn test_parse_ffi_call_lib_empty() {
        assert!(parse_ffi_call("ffi:").is_err());
    }

    #[test]
    fn test_parse_ffi_call_python_empty() {
        assert!(parse_ffi_call("python:").is_err());
    }

    #[test]
    fn test_parse_ffi_call_invalid() {
        assert!(parse_ffi_call("some_command").is_err());
    }

    #[test]
    fn test_parse_ffi_call_empty() {
        assert!(parse_ffi_call("").is_err());
    }

    #[test]
    fn test_ffi_signature() {
        let func = FfiConnector::new();
        let sig = func.signature();
        assert_eq!(sig.name, "ffi");
        // call is no longer in arg_specs — it's injected by source definition resolution
        assert_eq!(sig.arg_specs.len(), 0);
        assert!(sig.accepts_extra_args);
    }

    #[test]
    fn test_parse_discover_json() {
        let json = r#"{"locations": [
            {"location": "file1.parquet", "must_copy": true, "format": "parquet", "version": "v1", "num_rows": 100},
            {"location": "file2.csv", "must_copy": false, "format": "csv", "num_rows": null}
        ]}"#;
        let locations = parse_discover_json(json).expect("should parse");
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].location, "file1.parquet");
        assert!(locations[0].must_copy);
        assert_eq!(locations[0].format, SourceFormat::Parquet);
        assert_eq!(locations[0].version, "v1");
        assert_eq!(locations[1].location, "file2.csv");
        assert!(!locations[1].must_copy);
        assert_eq!(locations[1].format, SourceFormat::Csv);
        assert_eq!(locations[1].version, "");
    }

    #[test]
    fn test_filtered_args_json() {
        let mut args = HashMap::new();
        args.insert("call".to_string(), "ffi:test.so".to_string());
        args.insert("custom".to_string(), "value".to_string());

        let json = filtered_args_json(&args).expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse JSON");
        assert!(parsed.get("call").is_none());
        assert_eq!(parsed.get("custom").and_then(|v| v.as_str()), Some("value"));
    }

    #[test]
    fn test_location_json() {
        let loc = DiscoveredLocation {
            location: "test.parquet".to_string(),
            must_copy: true,
            format: SourceFormat::Parquet,
            version: "v1".to_string(),
            num_rows: None,
        };
        let json = location_json(&loc).expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("should parse JSON");
        assert_eq!(
            parsed.get("location").and_then(|v| v.as_str()),
            Some("test.parquet")
        );
    }

    // validate_args tests removed — call format validation now happens at import_connector time

    #[tokio::test]
    async fn test_discover_blocked_when_external_code_disabled() {
        let func = FfiConnector::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "python:mod:Class".to_string());
        let config: Arc<dyn bundlebase_common::ConfigProvider> = crate::test_utils::test_config();

        let result = func.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err
            .to_string()
            .contains("External code execution is disabled"));
    }

    #[tokio::test]
    async fn test_data_blocked_when_external_code_disabled() {
        let func = FfiConnector::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "python:mod:Class".to_string());
        let config: Arc<dyn bundlebase_common::ConfigProvider> = crate::test_utils::test_config();
        let location = DiscoveredLocation {
            location: "test.parquet".to_string(),
            must_copy: true,
            format: SourceFormat::Parquet,
            version: "v1".to_string(),
            num_rows: None,
        };

        let result = func.data(&location, &args, &config).await;
        let err = result.err().expect("should fail");
        assert!(err
            .to_string()
            .contains("External code execution is disabled"));
    }

    #[tokio::test]
    async fn test_stable_url_blocked_when_external_code_disabled() {
        let func = FfiConnector::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "python:mod:Class".to_string());
        let config: Arc<dyn bundlebase_common::ConfigProvider> = crate::test_utils::test_config();
        let location = DiscoveredLocation {
            location: "test.parquet".to_string(),
            must_copy: true,
            format: SourceFormat::Parquet,
            version: "v1".to_string(),
            num_rows: None,
        };

        let result = func.stable_url(&location, &args, &config).await;
        let err = result.err().expect("should fail");
        assert!(err
            .to_string()
            .contains("External code execution is disabled"));
    }

    #[tokio::test]
    async fn test_discover_allowed_with_config() {
        // With allow_external_code=true, discover should pass the config gate
        // (will still fail because the Python bridge isn't initialized, but the
        // error should NOT be about external code being disabled)
        let func = FfiConnector::new();
        let mut args = HashMap::new();
        args.insert("call".to_string(), "python:mod:Class".to_string());

        let config = crate::test_utils::test_config_with_values(&[(
            "system",
            "allow_external_code",
            "true",
        )]) as Arc<dyn bundlebase_common::ConfigProvider>;

        let result = func.discover(&args, &HashSet::new(), &config).await;
        assert!(result.is_err());
        let err = result.expect_err("should fail (no bridge)");
        assert!(!err
            .to_string()
            .contains("External code execution is disabled"));
    }

    // ----- structural shared-lib header verification -----

    fn write_tmp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "bb_verify_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            name
        ));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn fake_elf(e_machine: u16) -> Vec<u8> {
        let mut v = vec![0u8; 64];
        v[0..4].copy_from_slice(b"\x7FELF");
        v[18..20].copy_from_slice(&e_machine.to_le_bytes());
        v
    }

    fn fake_macho(cputype: u32) -> Vec<u8> {
        let mut v = vec![0u8; 32];
        v[0..4].copy_from_slice(&0xFEEDFACFu32.to_le_bytes());
        v[4..8].copy_from_slice(&cputype.to_le_bytes());
        v
    }

    fn fake_pe(machine: u16) -> Vec<u8> {
        let mut v = vec![0u8; 0x100];
        v[0..2].copy_from_slice(b"MZ");
        let pe_off: u32 = 0x80;
        v[0x3C..0x40].copy_from_slice(&pe_off.to_le_bytes());
        v[0x80..0x84].copy_from_slice(b"PE\0\0");
        v[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
        v
    }

    #[test]
    fn test_verify_header_elf_amd64_ok() {
        // ELF e_machine 0x3E = AMD64
        let p = write_tmp("good.so", &fake_elf(0x3E));
        verify_shared_lib_header(p.to_str().unwrap(), "linux", "amd64").unwrap();
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_elf_arm64_ok() {
        let p = write_tmp("good_arm.so", &fake_elf(0xB7));
        verify_shared_lib_header(p.to_str().unwrap(), "linux", "arm64").unwrap();
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_elf_arch_mismatch() {
        // amd64 ELF claimed as arm64
        let p = write_tmp("mismatch.so", &fake_elf(0x3E));
        let err = verify_shared_lib_header(p.to_str().unwrap(), "linux", "arm64").unwrap_err();
        assert!(
            err.to_string().contains("does not match linux/arm64"),
            "got: {}",
            err
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_elf_wildcard_arch_skips_check() {
        let p = write_tmp("wild.so", &fake_elf(0x3E));
        verify_shared_lib_header(p.to_str().unwrap(), "linux", "*").unwrap();
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_not_elf() {
        let p = write_tmp("notelf.so", b"not an elf file at all");
        let err = verify_shared_lib_header(p.to_str().unwrap(), "linux", "amd64").unwrap_err();
        assert!(err.to_string().contains("not an ELF"), "got: {}", err);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_macho_amd64_ok() {
        // CPU_TYPE_X86_64 = 0x01000007
        let p = write_tmp("good.dylib", &fake_macho(0x01000007));
        verify_shared_lib_header(p.to_str().unwrap(), "darwin", "amd64").unwrap();
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_macho_arm64_ok() {
        let p = write_tmp("good_arm.dylib", &fake_macho(0x0100000C));
        verify_shared_lib_header(p.to_str().unwrap(), "darwin", "arm64").unwrap();
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_macho_arch_mismatch() {
        let p = write_tmp("bad.dylib", &fake_macho(0x01000007));
        let err = verify_shared_lib_header(p.to_str().unwrap(), "darwin", "arm64").unwrap_err();
        assert!(
            err.to_string().contains("does not match darwin/arm64"),
            "got: {}",
            err
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_macho_fat_skips_arch() {
        // Fat binary big-endian magic 0xCAFEBABE
        let mut v = vec![0u8; 32];
        v[0..4].copy_from_slice(&0xCAFEBABEu32.swap_bytes().to_le_bytes());
        let p = write_tmp("fat.dylib", &v);
        verify_shared_lib_header(p.to_str().unwrap(), "darwin", "arm64").unwrap();
        verify_shared_lib_header(p.to_str().unwrap(), "darwin", "amd64").unwrap();
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_pe_amd64_ok() {
        let p = write_tmp("good.dll", &fake_pe(0x8664));
        verify_shared_lib_header(p.to_str().unwrap(), "windows", "amd64").unwrap();
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_pe_arm64_ok() {
        let p = write_tmp("good_arm.dll", &fake_pe(0xAA64));
        verify_shared_lib_header(p.to_str().unwrap(), "windows", "arm64").unwrap();
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_pe_machine_mismatch() {
        let p = write_tmp("bad.dll", &fake_pe(0x8664));
        let err = verify_shared_lib_header(p.to_str().unwrap(), "windows", "arm64").unwrap_err();
        assert!(
            err.to_string().contains("does not match windows/arm64"),
            "got: {}",
            err
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_not_pe() {
        let p = write_tmp("notpe.dll", b"this is not a PE binary");
        let err = verify_shared_lib_header(p.to_str().unwrap(), "windows", "amd64").unwrap_err();
        assert!(
            err.to_string().contains("not a PE/DLL") || err.to_string().contains("PE signature"),
            "got: {}",
            err
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_wildcard_os_rejected() {
        let p = write_tmp("any.so", &fake_elf(0x3E));
        let err = verify_shared_lib_header(p.to_str().unwrap(), "*", "amd64").unwrap_err();
        assert!(
            err.to_string().contains("wildcard os"),
            "got: {}",
            err
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_unsupported_os() {
        let p = write_tmp("any2.so", &fake_elf(0x3E));
        let err = verify_shared_lib_header(p.to_str().unwrap(), "freebsd", "amd64").unwrap_err();
        assert!(
            err.to_string().contains("Unsupported target os"),
            "got: {}",
            err
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify_header_missing_file() {
        let err = verify_shared_lib_header("/nonexistent/path/lib.so", "linux", "amd64")
            .unwrap_err();
        assert!(err.to_string().contains("Failed to read"), "got: {}", err);
    }
}
