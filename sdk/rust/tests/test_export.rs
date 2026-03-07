/// Integration tests for the `export_source!` macro.
///
/// These tests invoke the generated C ABI functions (bundlebase_discover,
/// bundlebase_data, bundlebase_free, bundlebase_stable_url) to verify
/// end-to-end correctness of the native export path.
use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ffi_stream::ArrowArrayStreamReader;
use arrow::ffi_stream::FFI_ArrowArrayStream;
use arrow::record_batch::RecordBatch;
use bundlebase_sdk::{export_source, Location, Connector, StableUrl};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::sync::Arc;

struct TestExportSource;

impl Connector for TestExportSource {
    fn discover(
        &self,
        _attached: &[String],
        args: &HashMap<String, String>,
    ) -> Result<Vec<Location>, Box<dyn std::error::Error>> {
        let mut locations = vec![Location::new("data.parquet")];
        locations[0].version = "v1".to_string();
        if let Some(extra) = args.get("extra") {
            locations.push(Location::new(extra.clone()));
        }
        Ok(locations)
    }

    fn data(
        &self,
        location: &Location,
        _args: &HashMap<String, String>,
    ) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
        if location.location == "data.parquet" {
            let schema = Arc::new(Schema::new(vec![
                Field::new("id", DataType::Int64, false),
                Field::new("name", DataType::Utf8, false),
            ]));
            let batch = RecordBatch::try_new(
                schema,
                vec![
                    Arc::new(Int64Array::from(vec![1, 2, 3])),
                    Arc::new(StringArray::from(vec!["a", "b", "c"])),
                ],
            )?;
            Ok(Some(vec![batch]))
        } else {
            Ok(None)
        }
    }

    fn stable_url(
        &self,
        location: &Location,
        _args: &HashMap<String, String>,
    ) -> Result<Option<StableUrl>, Box<dyn std::error::Error>> {
        if location.location == "data.parquet" {
            Ok(Some(StableUrl {
                url: "https://example.com/data.parquet".to_string(),
            }))
        } else {
            Ok(None)
        }
    }
}

export_source!(TestExportSource);

// Re-declare the generated extern "C" functions so we can call them from tests.
extern "C" {
    fn bundlebase_discover(
        args_json: *const std::os::raw::c_char,
        out_json: *mut *mut std::os::raw::c_char,
    ) -> i32;

    fn bundlebase_data(
        location_json: *const std::os::raw::c_char,
        args_json: *const std::os::raw::c_char,
        out: *mut FFI_ArrowArrayStream,
    ) -> i32;

    fn bundlebase_free(ptr: *mut std::os::raw::c_char);

    fn bundlebase_stable_url(
        location_json: *const std::os::raw::c_char,
        args_json: *const std::os::raw::c_char,
        out_json: *mut *mut std::os::raw::c_char,
    ) -> i32;
}

#[test]
fn test_discover_returns_locations() {
    let args = CString::new(r#"{"attached_locations": []}"#).unwrap();
    let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();

    let rc = unsafe { bundlebase_discover(args.as_ptr(), &mut out) };
    assert_eq!(rc, 0);
    assert!(!out.is_null());

    let json_str = unsafe { CStr::from_ptr(out) }.to_str().unwrap();
    let value: serde_json::Value = serde_json::from_str(json_str).unwrap();
    let locations = value["locations"].as_array().unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0]["location"], "data.parquet");
    assert_eq!(locations[0]["version"], "v1");

    unsafe { bundlebase_free(out) };
}

#[test]
fn test_discover_passes_extra_args() {
    let args = CString::new(r#"{"attached_locations": [], "extra": "bonus.csv"}"#).unwrap();
    let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();

    let rc = unsafe { bundlebase_discover(args.as_ptr(), &mut out) };
    assert_eq!(rc, 0);

    let json_str = unsafe { CStr::from_ptr(out) }.to_str().unwrap();
    let value: serde_json::Value = serde_json::from_str(json_str).unwrap();
    let locations = value["locations"].as_array().unwrap();
    assert_eq!(locations.len(), 2);
    assert_eq!(locations[1]["location"], "bonus.csv");

    unsafe { bundlebase_free(out) };
}

#[test]
fn test_discover_invalid_json() {
    let args = CString::new("not json").unwrap();
    let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();

    let rc = unsafe { bundlebase_discover(args.as_ptr(), &mut out) };
    assert_eq!(rc, -1);
    assert!(!out.is_null());

    // Error message should be set
    let err = unsafe { CStr::from_ptr(out) }.to_str().unwrap();
    assert!(err.contains("Invalid JSON"), "got: {}", err);

    unsafe { bundlebase_free(out) };
}

#[test]
fn test_data_returns_arrow_stream() {
    let loc = CString::new(
        r#"{"location": "data.parquet", "must_copy": true, "format": "parquet", "version": "v1"}"#,
    )
    .unwrap();
    let args = CString::new("{}").unwrap();

    let mut stream = FFI_ArrowArrayStream::empty();

    let rc = unsafe { bundlebase_data(loc.as_ptr(), args.as_ptr(), &mut stream) };
    assert_eq!(rc, 0);

    // Read the Arrow stream
    let reader = ArrowArrayStreamReader::try_new(stream).expect("valid stream");
    let batches: Vec<RecordBatch> = reader.collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].num_rows(), 3);
    assert_eq!(batches[0].num_columns(), 2);
}

#[test]
fn test_data_no_data_returns_empty() {
    let loc = CString::new(r#"{"location": "nonexistent"}"#).unwrap();
    let args = CString::new("{}").unwrap();

    let mut stream = FFI_ArrowArrayStream::empty();

    let rc = unsafe { bundlebase_data(loc.as_ptr(), args.as_ptr(), &mut stream) };
    assert_eq!(rc, 0);
    // Stream should remain empty (no reader populated)
}

#[test]
fn test_stable_url_present() {
    let loc = CString::new(
        r#"{"location": "data.parquet", "must_copy": true, "format": "parquet", "version": "v1"}"#,
    )
    .unwrap();
    let args = CString::new("{}").unwrap();
    let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();

    let rc = unsafe { bundlebase_stable_url(loc.as_ptr(), args.as_ptr(), &mut out) };
    assert_eq!(rc, 0);
    assert!(!out.is_null());

    let json_str = unsafe { CStr::from_ptr(out) }.to_str().unwrap();
    let value: serde_json::Value = serde_json::from_str(json_str).unwrap();
    assert_eq!(value["url"], "https://example.com/data.parquet");

    unsafe { bundlebase_free(out) };
}

#[test]
fn test_stable_url_none() {
    let loc = CString::new(r#"{"location": "other.parquet"}"#).unwrap();
    let args = CString::new("{}").unwrap();
    let mut out: *mut std::os::raw::c_char = std::ptr::null_mut();

    let rc = unsafe { bundlebase_stable_url(loc.as_ptr(), args.as_ptr(), &mut out) };
    assert_eq!(rc, 0);
    // out should remain null (no stable URL)
    assert!(out.is_null());
}

#[test]
fn test_free_null_is_safe() {
    // Should not crash
    unsafe { bundlebase_free(std::ptr::null_mut()) };
}
