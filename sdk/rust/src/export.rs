//! Macro for exporting a `SourceFunction` as a plugin shared library.
//!
//! Generates the `extern "C"` functions required by the Bundlebase plugin
//! source ABI:
//!
//! - `bundlebase_discover(args_json, out_json) -> i32`
//! - `bundlebase_data(location_json, args_json, out) -> i32`
//! - `bundlebase_free(ptr)`
//! - `bundlebase_stable_url(location_json, args_json, out_json) -> i32`
//!
//! # Usage
//!
//! In your `lib.rs`:
//!
//! ```rust,ignore
//! use bundlebase_sdk::{SourceFunction, Location, export_source};
//! use arrow::record_batch::RecordBatch;
//! use std::collections::HashMap;
//!
//! struct MySource;
//!
//! impl SourceFunction for MySource {
//!     fn discover(&self, attached: &[String], args: &HashMap<String, String>)
//!         -> Result<Vec<Location>, Box<dyn std::error::Error>> {
//!         Ok(vec![Location::new("data.parquet")])
//!     }
//!
//!     fn data(&self, location: &Location, args: &HashMap<String, String>)
//!         -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
//!         Ok(None) // return your data here
//!     }
//! }
//!
//! export_source!(MySource);
//! ```
//!
//! Build with `cargo build --release` (crate-type must include `"cdylib"`).
//! Then use from Bundlebase:
//!
//! ```python
//! bundle.create_source("plugin", {"call": "lib:target/release/libmy_source.so"})
//! ```

/// Generate the `extern "C"` entry points for a Bundlebase plugin source.
///
/// Pass an expression that creates a `SourceFunction`. The macro stores it
/// in a `OnceLock`-backed singleton and generates the four C ABI functions.
///
/// # Examples
///
/// ```rust,ignore
/// // With a unit struct
/// export_source!(MySource);
///
/// // With a constructor
/// export_source!(MySource::new());
///
/// // With configuration
/// export_source!(MySource::with_config("prod"));
/// ```
#[macro_export]
macro_rules! export_source {
    ($source_expr:expr) => {
        mod __bundlebase_export {
            use super::*;
            use std::collections::HashMap;
            use std::ffi::{CStr, CString};
            use std::os::raw::c_char;
            use std::sync::OnceLock;

            use ::arrow::ffi_stream::FFI_ArrowArrayStream;
            use ::arrow::record_batch::RecordBatch;
            use $crate::source::SourceFunction;
            use $crate::types::Location;

            static SOURCE: OnceLock<Box<dyn SourceFunction + Send + Sync>> = OnceLock::new();

            fn get_source() -> &'static (dyn SourceFunction + Send + Sync) {
                SOURCE
                    .get_or_init(|| Box::new($source_expr))
                    .as_ref()
            }

            fn alloc_c_string(s: &str) -> *mut c_char {
                match CString::new(s) {
                    Ok(cs) => cs.into_raw(),
                    Err(_) => {
                        // Strip null bytes and try again
                        let cleaned: String = s.chars().filter(|c| *c != '\0').collect();
                        CString::new(cleaned)
                            .unwrap_or_else(|_| CString::new("internal error").unwrap())
                            .into_raw()
                    }
                }
            }

            fn parse_args_json(json: &str) -> Result<(Vec<String>, HashMap<String, String>), String> {
                let value: serde_json::Value =
                    serde_json::from_str(json).map_err(|e| format!("Invalid JSON: {}", e))?;

                let attached: Vec<String> = value
                    .get("attached_locations")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();

                let mut args = HashMap::new();
                if let Some(obj) = value.as_object() {
                    for (k, v) in obj {
                        if k == "attached_locations" {
                            continue;
                        }
                        if let Some(s) = v.as_str() {
                            args.insert(k.clone(), s.to_string());
                        }
                    }
                }

                Ok((attached, args))
            }

            fn parse_location_json(json: &str) -> Result<Location, String> {
                serde_json::from_str(json).map_err(|e| format!("Invalid location JSON: {}", e))
            }

            fn parse_simple_args(json: &str) -> Result<HashMap<String, String>, String> {
                let value: serde_json::Value =
                    serde_json::from_str(json).map_err(|e| format!("Invalid JSON: {}", e))?;
                let mut args = HashMap::new();
                if let Some(obj) = value.as_object() {
                    for (k, v) in obj {
                        if let Some(s) = v.as_str() {
                            args.insert(k.clone(), s.to_string());
                        }
                    }
                }
                Ok(args)
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn bundlebase_discover(
                args_json: *const c_char,
                out_json: *mut *mut c_char,
            ) -> i32 {
                let json_str = match unsafe { CStr::from_ptr(args_json) }.to_str() {
                    Ok(s) => s,
                    Err(_) => {
                        if !out_json.is_null() {
                            unsafe { *out_json = alloc_c_string("Invalid UTF-8 in args_json") };
                        }
                        return -1;
                    }
                };

                let (attached, args) = match parse_args_json(json_str) {
                    Ok(v) => v,
                    Err(msg) => {
                        if !out_json.is_null() {
                            unsafe { *out_json = alloc_c_string(&msg) };
                        }
                        return -1;
                    }
                };

                let source = get_source();
                match source.discover(&attached, &args) {
                    Ok(locations) => {
                        let response = serde_json::json!({ "locations": locations });
                        let json = serde_json::to_string(&response).unwrap_or_default();
                        if !out_json.is_null() {
                            unsafe { *out_json = alloc_c_string(&json) };
                        }
                        0
                    }
                    Err(e) => {
                        if !out_json.is_null() {
                            unsafe { *out_json = alloc_c_string(&e.to_string()) };
                        }
                        -1
                    }
                }
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn bundlebase_data(
                location_json: *const c_char,
                args_json: *const c_char,
                out: *mut FFI_ArrowArrayStream,
            ) -> i32 {
                let loc_str = match unsafe { CStr::from_ptr(location_json) }.to_str() {
                    Ok(s) => s,
                    Err(_) => return -1,
                };
                let args_str = match unsafe { CStr::from_ptr(args_json) }.to_str() {
                    Ok(s) => s,
                    Err(_) => return -1,
                };

                let location = match parse_location_json(loc_str) {
                    Ok(l) => l,
                    Err(_) => return -1,
                };
                let args = match parse_simple_args(args_str) {
                    Ok(a) => a,
                    Err(_) => return -1,
                };

                let source = get_source();
                match source.data(&location, &args) {
                    Ok(Some(batches)) if !batches.is_empty() => {
                        let schema = batches[0].schema();
                        let reader = ::arrow::record_batch::RecordBatchIterator::new(
                            batches.into_iter().map(Ok),
                            schema,
                        );
                        let mut ffi_stream = FFI_ArrowArrayStream::new(Box::new(reader));
                        if !out.is_null() {
                            unsafe { std::ptr::write(out, ffi_stream) };
                        }
                        0
                    }
                    Ok(_) => {
                        // No data — leave stream empty (release == None)
                        0
                    }
                    Err(_) => -1,
                }
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn bundlebase_free(ptr: *mut c_char) {
                if !ptr.is_null() {
                    drop(unsafe { CString::from_raw(ptr) });
                }
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn bundlebase_stable_url(
                location_json: *const c_char,
                args_json: *const c_char,
                out_json: *mut *mut c_char,
            ) -> i32 {
                let loc_str = match unsafe { CStr::from_ptr(location_json) }.to_str() {
                    Ok(s) => s,
                    Err(_) => return -1,
                };
                let args_str = match unsafe { CStr::from_ptr(args_json) }.to_str() {
                    Ok(s) => s,
                    Err(_) => return -1,
                };

                let location = match parse_location_json(loc_str) {
                    Ok(l) => l,
                    Err(_) => return -1,
                };
                let args = match parse_simple_args(args_str) {
                    Ok(a) => a,
                    Err(_) => return -1,
                };

                let source = get_source();
                match source.stable_url(&location, &args) {
                    Ok(Some(stable_url)) => {
                        let json = serde_json::json!({ "url": stable_url.url });
                        let s = serde_json::to_string(&json).unwrap_or_default();
                        if !out_json.is_null() {
                            unsafe { *out_json = alloc_c_string(&s) };
                        }
                        0
                    }
                    Ok(None) => {
                        // null = no stable URL
                        0
                    }
                    Err(_) => -1,
                }
            }
        }
    };
}
