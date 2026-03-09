//! FFI bridge for loading and invoking native shared library (.so/.dylib) functions.
//!
//! Provides:
//! - `parse_lib_logic()` — splits `path:symbol` into path + optional symbol
//! - `load_library()` — loads and caches shared libraries via `libloading`
//! - `invoke_lib_scalar()` — calls a C scalar function through Arrow FFI
//! - `LibAccumulator` — wraps C aggregate function state for DataFusion
//! - `load_lib_manifest()` / `load_ipc_manifest()` — bulk function discovery

use crate::BundlebaseError;
use arrow::array::ArrayRef;
use arrow::datatypes::DataType;
use arrow::ffi::{FFI_ArrowArray, FFI_ArrowSchema};
use datafusion::scalar::ScalarValue;
use libloading::{Library, Symbol};
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::{Arc, Mutex};

/// Parse a lib/IPC logic string in `"path:symbol"` format.
///
/// The colon convention mirrors Python's `module:function` pattern.
/// If no colon is present, returns `None` for the symbol (caller uses default).
///
/// # Examples
/// - `"./mylib.so:double_val"` → `("./mylib.so", Some("double_val"))`
/// - `"./mylib.so"` → `("./mylib.so", None)`
pub fn parse_lib_logic(logic: &str) -> Result<(&str, Option<&str>), BundlebaseError> {
    if logic.is_empty() {
        return Err("Lib function logic string cannot be empty".into());
    }

    // Use rsplit to find the last colon, since paths may contain colons (e.g., Windows)
    if let Some(colon_pos) = logic.rfind(':') {
        let path = &logic[..colon_pos];
        let symbol = &logic[colon_pos + 1..];

        if path.is_empty() {
            return Err(format!(
                "Invalid lib function logic '{}'. Path before ':' cannot be empty.",
                logic
            )
            .into());
        }
        if symbol.is_empty() {
            return Err(format!(
                "Invalid lib function logic '{}'. Symbol after ':' cannot be empty.",
                logic
            )
            .into());
        }

        Ok((path, Some(symbol)))
    } else {
        Ok((logic, None))
    }
}

/// Global cache of loaded shared libraries.
///
/// Libraries are kept open for the process lifetime to avoid reloading.
static LIB_CACHE: std::sync::LazyLock<Mutex<HashMap<String, Arc<Library>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Load a shared library, using a cache to avoid reloading.
///
/// # Safety
/// Loading a shared library executes its init functions and trusts the library code.
pub fn load_library(path: &str) -> Result<Arc<Library>, BundlebaseError> {
    let mut cache = LIB_CACHE.lock().map_err(|e| {
        BundlebaseError::from(format!("Failed to acquire library cache lock: {}", e))
    })?;

    if let Some(lib) = cache.get(path) {
        return Ok(Arc::clone(lib));
    }

    let lib = unsafe { Library::new(path) }.map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to load shared library '{}': {}",
            path, e
        ))
    })?;

    let lib = Arc::new(lib);
    cache.insert(path.to_string(), Arc::clone(&lib));
    Ok(lib)
}

/// C function signature for scalar UDFs.
///
/// Returns 0 on success, non-zero on error.
type ScalarFn = unsafe extern "C" fn(
    args: *const *const FFI_ArrowArray,
    arg_schemas: *const *const FFI_ArrowSchema,
    n_args: i64,
    out_array: *mut FFI_ArrowArray,
    out_schema: *mut FFI_ArrowSchema,
    err_buf: *mut u8,
    err_buf_len: i64,
) -> i32;

/// Invoke a scalar function from a shared library.
///
/// Converts Arrow arrays to FFI, calls the C function, converts back.
pub fn invoke_lib_scalar(
    lib_path: &str,
    symbol: &str,
    args: &[ArrayRef],
) -> Result<ArrayRef, BundlebaseError> {
    let lib = load_library(lib_path)?;

    let func: Symbol<ScalarFn> = unsafe { lib.get(symbol.as_bytes()) }.map_err(|e| {
        BundlebaseError::from(format!(
            "Symbol '{}' not found in '{}': {}",
            symbol, lib_path, e
        ))
    })?;

    // Convert each ArrayRef to FFI structs
    let mut ffi_arrays: Vec<FFI_ArrowArray> = Vec::with_capacity(args.len());
    let mut ffi_schemas: Vec<FFI_ArrowSchema> = Vec::with_capacity(args.len());

    for arg in args {
        let data = arg.to_data();
        let (ffi_array, ffi_schema) = arrow::ffi::to_ffi(&data).map_err(|e| {
            BundlebaseError::from(format!("Failed to convert arg to FFI: {}", e))
        })?;
        ffi_arrays.push(ffi_array);
        ffi_schemas.push(ffi_schema);
    }

    // Build pointer arrays
    let arg_ptrs: Vec<*const FFI_ArrowArray> = ffi_arrays.iter().map(|a| a as *const _).collect();
    let schema_ptrs: Vec<*const FFI_ArrowSchema> =
        ffi_schemas.iter().map(|s| s as *const _).collect();

    // Prepare output
    let mut out_array = FFI_ArrowArray::empty();
    let mut out_schema = FFI_ArrowSchema::empty();

    // Error buffer
    let mut err_buf = vec![0u8; 1024];

    let rc = unsafe {
        func(
            arg_ptrs.as_ptr(),
            schema_ptrs.as_ptr(),
            args.len() as i64,
            &mut out_array,
            &mut out_schema,
            err_buf.as_mut_ptr(),
            err_buf.len() as i64,
        )
    };

    // The C function consumed the input FFI arrays via ptr::read + from_ffi.
    // We must forget them to prevent double-free (release callback already called).
    for ffi_array in ffi_arrays {
        std::mem::forget(ffi_array);
    }

    if rc != 0 {
        let err_msg = extract_c_error(&err_buf);
        return Err(format!(
            "Lib scalar function '{}' in '{}' failed (rc={}): {}",
            symbol, lib_path, rc, err_msg
        )
        .into());
    }

    // Convert FFI output back to ArrayRef
    let out_data =
        unsafe { arrow::ffi::from_ffi(out_array, &out_schema) }.map_err(|e| {
            BundlebaseError::from(format!("Failed to convert FFI output: {}", e))
        })?;

    Ok(arrow::array::make_array(out_data))
}

/// Extract a null-terminated C string from a buffer.
fn extract_c_error(buf: &[u8]) -> String {
    match buf.iter().position(|&b| b == 0) {
        Some(nul_pos) => String::from_utf8_lossy(&buf[..nul_pos]).to_string(),
        None => String::from_utf8_lossy(buf).to_string(),
    }
}

// ==================== Aggregate function C ABI types ====================

type CreateStateFn = unsafe extern "C" fn() -> *mut std::ffi::c_void;

type AccumulateFn = unsafe extern "C" fn(
    state: *mut std::ffi::c_void,
    args: *const *const FFI_ArrowArray,
    arg_schemas: *const *const FFI_ArrowSchema,
    n_args: i64,
    err_buf: *mut u8,
    err_buf_len: i64,
) -> i32;

type EvaluateFn = unsafe extern "C" fn(
    state: *mut std::ffi::c_void,
    out_array: *mut FFI_ArrowArray,
    out_schema: *mut FFI_ArrowSchema,
    err_buf: *mut u8,
    err_buf_len: i64,
) -> i32;

type FreeStateFn = unsafe extern "C" fn(state: *mut std::ffi::c_void);

/// Accumulator backed by a native shared library aggregate function.
///
/// Wraps an opaque `void*` state pointer and delegates to the C ABI symbols.
pub struct LibAccumulator {
    lib: Arc<Library>,
    lib_path: String,
    symbol: String,
    state: *mut std::ffi::c_void,
    #[allow(dead_code)]
    return_type: DataType,
}

// Safety: The C library manages its own state; we only pass the pointer across threads.
// The void* state is only accessed through &mut self methods.
unsafe impl Send for LibAccumulator {}
unsafe impl Sync for LibAccumulator {}

impl std::fmt::Debug for LibAccumulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibAccumulator")
            .field("lib_path", &self.lib_path)
            .field("symbol", &self.symbol)
            .finish()
    }
}

impl LibAccumulator {
    /// Create a new accumulator by calling `<symbol>_create_state()`.
    pub fn new(lib_path: &str, symbol: &str, return_type: DataType) -> Result<Self, BundlebaseError> {
        let lib = load_library(lib_path)?;
        let create_sym = format!("{}_create_state", symbol);

        let create_fn: Symbol<CreateStateFn> =
            unsafe { lib.get(create_sym.as_bytes()) }.map_err(|e| {
                BundlebaseError::from(format!(
                    "Symbol '{}' not found in '{}': {}",
                    create_sym, lib_path, e
                ))
            })?;

        let state = unsafe { create_fn() };

        Ok(Self {
            lib,
            lib_path: lib_path.to_string(),
            symbol: symbol.to_string(),
            state,
            return_type,
        })
    }

    fn get_symbol<T>(&self, suffix: &str) -> Result<Symbol<'_, T>, BundlebaseError> {
        let sym_name = format!("{}_{}", self.symbol, suffix);
        unsafe { self.lib.get(sym_name.as_bytes()) }.map_err(|e| {
            BundlebaseError::from(format!(
                "Symbol '{}' not found in '{}': {}",
                sym_name, self.lib_path, e
            ))
        })
    }
}

impl datafusion::logical_expr::Accumulator for LibAccumulator {
    fn update_batch(&mut self, values: &[ArrayRef]) -> datafusion::common::Result<()> {
        let accumulate_fn: Symbol<AccumulateFn> =
            self.get_symbol("accumulate").map_err(|e| {
                datafusion::common::DataFusionError::Execution(e.to_string())
            })?;

        let mut ffi_arrays: Vec<FFI_ArrowArray> = Vec::with_capacity(values.len());
        let mut ffi_schemas: Vec<FFI_ArrowSchema> = Vec::with_capacity(values.len());

        for val in values {
            let data = val.to_data();
            let (ffi_array, ffi_schema) = arrow::ffi::to_ffi(&data).map_err(|e| {
                datafusion::common::DataFusionError::Execution(format!(
                    "Failed to convert arg to FFI: {}",
                    e
                ))
            })?;
            ffi_arrays.push(ffi_array);
            ffi_schemas.push(ffi_schema);
        }

        let arg_ptrs: Vec<*const FFI_ArrowArray> =
            ffi_arrays.iter().map(|a| a as *const _).collect();
        let schema_ptrs: Vec<*const FFI_ArrowSchema> =
            ffi_schemas.iter().map(|s| s as *const _).collect();

        let mut err_buf = vec![0u8; 1024];

        let rc = unsafe {
            accumulate_fn(
                self.state,
                arg_ptrs.as_ptr(),
                schema_ptrs.as_ptr(),
                values.len() as i64,
                err_buf.as_mut_ptr(),
                err_buf.len() as i64,
            )
        };

        // C function consumed the FFI arrays; prevent double-free
        for ffi_array in ffi_arrays {
            std::mem::forget(ffi_array);
        }

        if rc != 0 {
            let err_msg = extract_c_error(&err_buf);
            return Err(datafusion::common::DataFusionError::Execution(format!(
                "Lib accumulate for '{}' failed: {}",
                self.symbol, err_msg
            )));
        }

        Ok(())
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> datafusion::common::Result<()> {
        // For lib aggregates, merge is optional. If the symbol doesn't exist,
        // we fall back to treating each state array element as a batch to accumulate.
        if states.is_empty() {
            return Ok(());
        }
        self.update_batch(states)
    }

    fn evaluate(&mut self) -> datafusion::common::Result<ScalarValue> {
        let evaluate_fn: Symbol<EvaluateFn> = self.get_symbol("evaluate").map_err(|e| {
            datafusion::common::DataFusionError::Execution(e.to_string())
        })?;

        let mut out_array = FFI_ArrowArray::empty();
        let mut out_schema = FFI_ArrowSchema::empty();
        let mut err_buf = vec![0u8; 1024];

        let rc = unsafe {
            evaluate_fn(
                self.state,
                &mut out_array,
                &mut out_schema,
                err_buf.as_mut_ptr(),
                err_buf.len() as i64,
            )
        };

        if rc != 0 {
            let err_msg = extract_c_error(&err_buf);
            return Err(datafusion::common::DataFusionError::Execution(format!(
                "Lib evaluate for '{}' failed: {}",
                self.symbol, err_msg
            )));
        }

        let out_data = unsafe { arrow::ffi::from_ffi(out_array, &out_schema) }.map_err(|e| {
            datafusion::common::DataFusionError::Execution(format!(
                "Failed to convert FFI output: {}",
                e
            ))
        })?;

        let array = arrow::array::make_array(out_data);
        if array.len() != 1 {
            return Err(datafusion::common::DataFusionError::Execution(format!(
                "Lib evaluate for '{}' returned {} elements, expected 1",
                self.symbol,
                array.len()
            )));
        }

        ScalarValue::try_from_array(&array, 0)
    }

    fn state(&mut self) -> datafusion::common::Result<Vec<ScalarValue>> {
        // For simple aggregates, the state IS the current running value
        Ok(vec![self.evaluate()?])
    }

    fn size(&self) -> usize {
        std::mem::size_of::<Self>() + self.lib_path.len() + self.symbol.len()
    }
}

impl Drop for LibAccumulator {
    fn drop(&mut self) {
        if !self.state.is_null() {
            let free_sym = format!("{}_free_state", self.symbol);
            if let Ok(free_fn) = unsafe {
                self.lib.get::<FreeStateFn>(free_sym.as_bytes())
            } {
                unsafe { free_fn(self.state) };
            }
        }
    }
}

// ==================== Manifest discovery ====================

/// A single function entry from a manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct ManifestEntry {
    pub name: String,
    #[serde(default)]
    pub symbol: Option<String>,
    pub input_types: Vec<String>,
    pub return_type: String,
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "scalar".to_string()
}

/// JSON manifest returned by `bundlebase_functions()`.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub functions: Vec<ManifestEntry>,
}

/// C function signature for the manifest function.
type ManifestFn = unsafe extern "C" fn() -> *const std::ffi::c_char;

/// C function to free the manifest string.
type FreeManifestFn = unsafe extern "C" fn(ptr: *const std::ffi::c_char);

/// Load a function manifest from a shared library.
///
/// Calls the `bundlebase_functions()` C symbol, parses the returned JSON.
pub fn load_lib_manifest(lib_path: &str) -> Result<Manifest, BundlebaseError> {
    let lib = load_library(lib_path)?;

    let manifest_fn: Symbol<ManifestFn> =
        unsafe { lib.get(b"bundlebase_functions") }.map_err(|e| {
            BundlebaseError::from(format!(
                "Symbol 'bundlebase_functions' not found in '{}': {}. \
                 Library must export this function for bulk discovery.",
                lib_path, e
            ))
        })?;

    let ptr = unsafe { manifest_fn() };
    if ptr.is_null() {
        return Err(format!(
            "bundlebase_functions() in '{}' returned null",
            lib_path
        )
        .into());
    }

    let json_str = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|e| {
            BundlebaseError::from(format!("Invalid UTF-8 from bundlebase_functions(): {}", e))
        })?
        .to_string();

    // Free the manifest string if the library provides a free function
    if let Ok(free_fn) = unsafe {
        lib.get::<FreeManifestFn>(b"bundlebase_free_manifest")
    } {
        unsafe { free_fn(ptr) };
    }

    let manifest: Manifest = serde_json::from_str(&json_str).map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to parse manifest JSON from '{}': {}. JSON: {}",
            lib_path, e, json_str
        ))
    })?;

    Ok(manifest)
}

/// Load a function manifest from an IPC executable.
///
/// Runs `exec_path --bundlebase-functions`, captures stdout, parses JSON.
pub fn load_ipc_manifest(exec_path: &str) -> Result<Manifest, BundlebaseError> {
    let output = std::process::Command::new(exec_path)
        .arg("--bundlebase-functions")
        .output()
        .map_err(|e| {
            BundlebaseError::from(format!(
                "Failed to execute '{}' for manifest discovery: {}",
                exec_path, e
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "'{}' --bundlebase-functions failed (exit {}): {}",
            exec_path,
            output.status,
            stderr.trim()
        )
        .into());
    }

    let json_str = String::from_utf8(output.stdout).map_err(|e| {
        BundlebaseError::from(format!(
            "Invalid UTF-8 output from '{}' --bundlebase-functions: {}",
            exec_path, e
        ))
    })?;

    let manifest: Manifest = serde_json::from_str(&json_str).map_err(|e| {
        BundlebaseError::from(format!(
            "Failed to parse manifest JSON from '{}': {}. Output: {}",
            exec_path, e, json_str.trim()
        ))
    })?;

    Ok(manifest)
}

/// Look up a single function's metadata from its runner and logic string.
///
/// Supports all runners:
/// - `Lib` → loads the shared library manifest via `bundlebase_functions()` C symbol
/// - `Ipc` / `Java` / `Docker` → runs `exec --bundlebase-functions` and parses JSON
/// - `Python` → calls `bundlebase_metadata()` on the Python module via the bridge
///
/// Returns the matching `ManifestEntry` or an error if the function is not found.
pub fn lookup_function_in_manifest(
    runner: &crate::bundle::connector_definition::Runner,
    logic: &str,
    function_name: &str,
) -> Result<ManifestEntry, BundlebaseError> {
    use crate::bundle::connector_definition::Runner;

    let entries = match runner {
        Runner::Lib => {
            let (lib_path, _) = parse_lib_logic(logic)?;
            let manifest = load_lib_manifest(lib_path)?;
            manifest.functions
        }
        Runner::Ipc | Runner::Java | Runner::Docker => {
            // For IPC-based runners, the logic string is the executable/call string.
            // We need to extract just the executable path for --bundlebase-functions.
            let (exec_path, _) = parse_lib_logic(logic)?;
            let manifest = load_ipc_manifest(exec_path)?;
            manifest.functions
        }
        Runner::Python => {
            // Parse module from logic string (module:function format)
            let colon_pos = logic.rfind(':').ok_or_else(|| {
                BundlebaseError::from(format!(
                    "Invalid Python logic '{}'. Expected 'module:function' format.",
                    logic
                ))
            })?;
            let module = &logic[..colon_pos];

            let bridge = crate::function::python_bridge::get_python_function_bridge()?;
            match bridge.get_function_metadata(module)? {
                Some(entries) => entries,
                None => {
                    return Err(format!(
                        "Python module '{}' does not define bundlebase_metadata(). \
                         Provide explicit type signatures or add a bundlebase_metadata() function.",
                        module
                    )
                    .into());
                }
            }
        }
    };

    // Look up by function name (short name without namespace)
    entries
        .into_iter()
        .find(|e| e.name == function_name)
        .ok_or_else(|| {
            format!(
                "Function '{}' not found in manifest from '{}'. \
                 Available functions: check the manifest or provide explicit type signatures.",
                function_name, logic
            )
            .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== parse_lib_logic tests ====================

    #[test]
    fn test_parse_lib_logic_with_symbol() {
        let (path, symbol) = parse_lib_logic("./mylib.so:double_val").unwrap();
        assert_eq!(path, "./mylib.so");
        assert_eq!(symbol, Some("double_val"));
    }

    #[test]
    fn test_parse_lib_logic_without_symbol() {
        let (path, symbol) = parse_lib_logic("./mylib.so").unwrap();
        assert_eq!(path, "./mylib.so");
        assert_eq!(symbol, None);
    }

    #[test]
    fn test_parse_lib_logic_relative_path() {
        let (path, symbol) = parse_lib_logic("libs/mylib.dylib:func_name").unwrap();
        assert_eq!(path, "libs/mylib.dylib");
        assert_eq!(symbol, Some("func_name"));
    }

    #[test]
    fn test_parse_lib_logic_absolute_path() {
        let (path, symbol) = parse_lib_logic("/usr/local/lib/mylib.so:my_func").unwrap();
        assert_eq!(path, "/usr/local/lib/mylib.so");
        assert_eq!(symbol, Some("my_func"));
    }

    #[test]
    fn test_parse_lib_logic_empty() {
        assert!(parse_lib_logic("").is_err());
    }

    #[test]
    fn test_parse_lib_logic_empty_path() {
        let result = parse_lib_logic(":symbol");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Path before ':' cannot be empty"));
    }

    #[test]
    fn test_parse_lib_logic_empty_symbol() {
        let result = parse_lib_logic("./mylib.so:");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Symbol after ':' cannot be empty"));
    }

    #[test]
    fn test_parse_lib_logic_ipc_path() {
        let (path, symbol) = parse_lib_logic("./my_func:double_val").unwrap();
        assert_eq!(path, "./my_func");
        assert_eq!(symbol, Some("double_val"));
    }

    // ==================== manifest JSON parsing tests ====================

    #[test]
    fn test_manifest_deserialize() {
        let json = r#"{"functions": [
            {"name": "double_val", "symbol": "double_val",
             "input_types": ["Int64"], "return_type": "Int64", "kind": "scalar"},
            {"name": "my_sum", "input_types": ["Int64"],
             "return_type": "Int64", "kind": "aggregate"}
        ]}"#;

        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.functions.len(), 2);

        assert_eq!(manifest.functions[0].name, "double_val");
        assert_eq!(manifest.functions[0].symbol, Some("double_val".to_string()));
        assert_eq!(manifest.functions[0].input_types, vec!["Int64"]);
        assert_eq!(manifest.functions[0].return_type, "Int64");
        assert_eq!(manifest.functions[0].kind, "scalar");

        assert_eq!(manifest.functions[1].name, "my_sum");
        assert_eq!(manifest.functions[1].symbol, None);
        assert_eq!(manifest.functions[1].kind, "aggregate");
    }

    #[test]
    fn test_manifest_default_kind() {
        let json = r#"{"functions": [
            {"name": "double_val", "input_types": ["Int64"], "return_type": "Int64"}
        ]}"#;

        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.functions[0].kind, "scalar");
    }

    #[test]
    fn test_manifest_multi_input() {
        let json = r#"{"functions": [
            {"name": "add", "input_types": ["Int64", "Int64"], "return_type": "Int64"}
        ]}"#;

        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.functions[0].input_types, vec!["Int64", "Int64"]);
    }

    // ==================== Integration tests with test cdylib ====================
    //
    // These tests require the test-lib-function cdylib to be built first.
    // Run: cd rust/bundlebase/tests/test_lib_function && cargo build

    /// Get the path to the test cdylib, building it if needed.
    fn test_lib_path() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let test_lib_dir = format!("{}/tests/test_lib_function", manifest_dir);

        // Build the test library
        let status = std::process::Command::new("cargo")
            .arg("build")
            .current_dir(&test_lib_dir)
            .status()
            .expect("Failed to build test lib");
        assert!(status.success(), "Test lib build failed");

        #[cfg(target_os = "macos")]
        let lib_name = "libtest_lib_function.dylib";
        #[cfg(target_os = "linux")]
        let lib_name = "libtest_lib_function.so";
        #[cfg(target_os = "windows")]
        let lib_name = "test_lib_function.dll";

        format!("{}/target/debug/{}", test_lib_dir, lib_name)
    }

    #[test]
    fn test_load_library() {
        let path = test_lib_path();
        let lib = load_library(&path);
        assert!(lib.is_ok(), "Failed to load test lib: {:?}", lib.err());
    }

    #[test]
    fn test_invoke_lib_scalar_double_val() {
        let path = test_lib_path();
        let input: ArrayRef = Arc::new(arrow::array::Int64Array::from(vec![1, 2, 3, 4, 5]));
        let result = invoke_lib_scalar(&path, "double_val", &[input]).unwrap();

        let int_result = result
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .expect("Expected Int64Array");

        assert_eq!(int_result.values(), &[2, 4, 6, 8, 10]);
    }

    #[test]
    fn test_invoke_lib_scalar_double_val_with_nulls() {
        let path = test_lib_path();
        let input: ArrayRef = Arc::new(arrow::array::Int64Array::from(vec![
            Some(10),
            None,
            Some(30),
        ]));
        let result = invoke_lib_scalar(&path, "double_val", &[input]).unwrap();

        let int_result = result
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .expect("Expected Int64Array");

        use arrow::array::Array;
        assert_eq!(int_result.value(0), 20);
        assert!(int_result.to_data().is_null(1));
        assert_eq!(int_result.value(2), 60);
    }

    #[test]
    fn test_invoke_lib_scalar_symbol_not_found() {
        let path = test_lib_path();
        let input: ArrayRef = Arc::new(arrow::array::Int64Array::from(vec![1]));
        let result = invoke_lib_scalar(&path, "nonexistent_func", &[input]);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Symbol 'nonexistent_func' not found"));
    }

    #[test]
    fn test_lib_accumulator_sum() {
        use datafusion::logical_expr::Accumulator;

        let path = test_lib_path();
        let mut acc =
            LibAccumulator::new(&path, "int_sum", DataType::Int64).expect("create accumulator");

        let batch1: ArrayRef = Arc::new(arrow::array::Int64Array::from(vec![1, 2, 3]));
        acc.update_batch(&[batch1]).expect("accumulate batch 1");

        let batch2: ArrayRef = Arc::new(arrow::array::Int64Array::from(vec![4, 5]));
        acc.update_batch(&[batch2]).expect("accumulate batch 2");

        let result = acc.evaluate().expect("evaluate");
        match result {
            ScalarValue::Int64(Some(val)) => assert_eq!(val, 15),
            other => panic!("Expected Int64(15), got {:?}", other),
        }
    }

    #[test]
    fn test_load_lib_manifest() {
        let path = test_lib_path();
        let manifest = load_lib_manifest(&path).expect("load manifest");

        assert_eq!(manifest.functions.len(), 2);
        assert_eq!(manifest.functions[0].name, "double_val");
        assert_eq!(manifest.functions[0].kind, "scalar");
        assert_eq!(manifest.functions[1].name, "int_sum");
        assert_eq!(manifest.functions[1].kind, "aggregate");
    }
}
