//! Test cdylib for bundlebase lib runner integration tests.
//!
//! Exports:
//! - `double_val`: scalar function that doubles Int64 values
//! - `int_sum`: aggregate function that sums Int64 values
//! - `bundlebase_functions`: manifest discovery function
//!
//! ## ABI Convention
//! Input FFI arrays are borrowed (not consumed). The caller retains ownership.
//! Output FFI arrays are written to caller-provided buffers and the caller takes ownership.

use arrow::array::{ArrayRef, Int64Array};
use arrow::ffi::{FFI_ArrowArray, FFI_ArrowSchema};
use std::ffi::{c_char, c_void, CString};
use std::sync::Arc;

// ==================== Scalar: double_val ====================

/// Doubles each Int64 value in the input array.
///
/// # Safety
/// Called via FFI. Caller must provide valid pointers.
#[no_mangle]
pub unsafe extern "C" fn double_val(
    args: *const *const FFI_ArrowArray,
    arg_schemas: *const *const FFI_ArrowSchema,
    n_args: i64,
    out_array: *mut FFI_ArrowArray,
    out_schema: *mut FFI_ArrowSchema,
    err_buf: *mut u8,
    err_buf_len: i64,
) -> i32 {
    match double_val_impl(args, arg_schemas, n_args) {
        Ok((ffi_array, ffi_schema)) => {
            std::ptr::write(out_array, ffi_array);
            std::ptr::write(out_schema, ffi_schema);
            0
        }
        Err(msg) => {
            write_error(err_buf, err_buf_len, &msg);
            1
        }
    }
}

unsafe fn double_val_impl(
    args: *const *const FFI_ArrowArray,
    arg_schemas: *const *const FFI_ArrowSchema,
    n_args: i64,
) -> Result<(FFI_ArrowArray, FFI_ArrowSchema), String> {
    if n_args != 1 {
        return Err(format!("double_val expects 1 argument, got {}", n_args));
    }

    let arg_array_ptr = *args.offset(0);
    let arg_schema_ptr = *arg_schemas.offset(0);

    // Import WITHOUT taking ownership: we read the array but must not free it.
    // from_ffi takes ownership and will call release on drop, so we need to
    // prevent the original from being released. We do this by reading a copy.
    let ffi_array_copy = std::ptr::read(arg_array_ptr);
    let data = arrow::ffi::from_ffi(ffi_array_copy, &*arg_schema_ptr)
        .map_err(|e| format!("Failed to import arg: {}", e))?;

    let array = arrow::array::make_array(data);
    let int_array = array
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or("Expected Int64Array")?;

    let result: Int64Array = int_array.iter().map(|v| v.map(|x| x * 2)).collect();
    let result_ref: ArrayRef = Arc::new(result);

    let result_data = result_ref.to_data();
    let (ffi_array, ffi_schema) =
        arrow::ffi::to_ffi(&result_data).map_err(|e| format!("Failed to export: {}", e))?;

    Ok((ffi_array, ffi_schema))
}

// ==================== Aggregate: int_sum ====================

struct SumState {
    sum: i64,
}

#[no_mangle]
pub extern "C" fn int_sum_create_state() -> *mut c_void {
    let state = Box::new(SumState { sum: 0 });
    Box::into_raw(state) as *mut c_void
}

/// # Safety
/// Called via FFI. `state` must be a valid pointer from `int_sum_create_state`.
#[no_mangle]
pub unsafe extern "C" fn int_sum_accumulate(
    state: *mut c_void,
    args: *const *const FFI_ArrowArray,
    arg_schemas: *const *const FFI_ArrowSchema,
    n_args: i64,
    err_buf: *mut u8,
    err_buf_len: i64,
) -> i32 {
    match int_sum_accumulate_impl(state, args, arg_schemas, n_args) {
        Ok(()) => 0,
        Err(msg) => {
            write_error(err_buf, err_buf_len, &msg);
            1
        }
    }
}

unsafe fn int_sum_accumulate_impl(
    state: *mut c_void,
    args: *const *const FFI_ArrowArray,
    arg_schemas: *const *const FFI_ArrowSchema,
    n_args: i64,
) -> Result<(), String> {
    if n_args != 1 {
        return Err(format!("int_sum expects 1 argument, got {}", n_args));
    }

    let state = &mut *(state as *mut SumState);

    let arg_array_ptr = *args.offset(0);
    let arg_schema_ptr = *arg_schemas.offset(0);

    let ffi_array_copy = std::ptr::read(arg_array_ptr);
    let data = arrow::ffi::from_ffi(ffi_array_copy, &*arg_schema_ptr)
        .map_err(|e| format!("Failed to import arg: {}", e))?;

    let array = arrow::array::make_array(data);
    let int_array = array
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or("Expected Int64Array")?;

    for v in int_array.iter().flatten() {
        state.sum += v;
    }

    Ok(())
}

/// # Safety
/// Called via FFI. `state` must be a valid pointer from `int_sum_create_state`.
#[no_mangle]
pub unsafe extern "C" fn int_sum_evaluate(
    state: *mut c_void,
    out_array: *mut FFI_ArrowArray,
    out_schema: *mut FFI_ArrowSchema,
    err_buf: *mut u8,
    err_buf_len: i64,
) -> i32 {
    let state = &*(state as *mut SumState);

    let result: ArrayRef = Arc::new(Int64Array::from(vec![state.sum]));
    let result_data = result.to_data();

    match arrow::ffi::to_ffi(&result_data) {
        Ok((ffi_array, ffi_schema)) => {
            std::ptr::write(out_array, ffi_array);
            std::ptr::write(out_schema, ffi_schema);
            0
        }
        Err(e) => {
            write_error(err_buf, err_buf_len, &format!("Failed to export: {}", e));
            1
        }
    }
}

/// # Safety
/// Called via FFI. `state` must be a valid pointer from `int_sum_create_state`.
#[no_mangle]
pub unsafe extern "C" fn int_sum_free_state(state: *mut c_void) {
    if !state.is_null() {
        drop(Box::from_raw(state as *mut SumState));
    }
}

// ==================== Manifest ====================

const MANIFEST_JSON: &str = r#"{"functions": [
    {"name": "double_val", "symbol": "double_val", "input_types": ["Int64"], "return_type": "Int64", "kind": "scalar"},
    {"name": "int_sum", "symbol": "int_sum", "input_types": ["Int64"], "return_type": "Int64", "kind": "aggregate"}
]}"#;

#[no_mangle]
pub extern "C" fn bundlebase_functions() -> *const c_char {
    let c_str = CString::new(MANIFEST_JSON).expect("CString creation failed");
    c_str.into_raw() as *const c_char
}

/// # Safety
/// `ptr` must be a pointer returned by `bundlebase_functions`.
#[no_mangle]
pub unsafe extern "C" fn bundlebase_free_manifest(ptr: *const c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr as *mut c_char));
    }
}

// ==================== Helpers ====================

unsafe fn write_error(buf: *mut u8, buf_len: i64, msg: &str) {
    let bytes = msg.as_bytes();
    let copy_len = std::cmp::min(bytes.len(), (buf_len - 1) as usize);
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, copy_len);
    *buf.add(copy_len) = 0; // null terminate
}
