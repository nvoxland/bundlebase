# Native Functions (Lib & IPC Runners)

Native functions let you extend bundlebase's SQL with high-performance scalar and aggregate functions written in any language that can produce a shared library (.so/.dylib) or standalone executable.

## Runners

| Runner | How it works | Best for |
|--------|-------------|----------|
| `lib` | Loads a shared library (.so/.dylib) via FFI | Rust, C, C++ — zero IPC overhead |
| `ipc` | Launches an executable, communicates via Arrow IPC | Go, Python, any language |

## Writing a Lib Function (Rust/C)

### Scalar Function C ABI

A scalar function receives Arrow arrays and produces an Arrow array:

```c
int32_t <symbol_name>(
    const FFI_ArrowArray** args,         // Input arrays
    const FFI_ArrowSchema** arg_schemas, // Input schemas
    int64_t n_args,                      // Number of arguments
    FFI_ArrowArray* out_array,           // Output array (caller-allocated)
    FFI_ArrowSchema* out_schema,         // Output schema (caller-allocated)
    char* err_buf,                       // Error message buffer
    int64_t err_buf_len                  // Error buffer length
);
// Returns 0 on success, non-zero on error
```

**Ownership convention:** The function **consumes** input arrays (takes ownership via the FFI release callback). Output arrays are written to caller-provided buffers; the caller takes ownership.

### Aggregate Function C ABI

Aggregate functions use a state-based protocol with five symbols:

```c
// Create initial accumulator state
void* <symbol>_create_state();

// Accumulate a batch of values into state
int32_t <symbol>_accumulate(
    void* state,
    const FFI_ArrowArray** args,
    const FFI_ArrowSchema** arg_schemas,
    int64_t n_args,
    char* err_buf, int64_t err_buf_len
);

// Produce final result (single-element array)
int32_t <symbol>_evaluate(
    void* state,
    FFI_ArrowArray* out_array,
    FFI_ArrowSchema* out_schema,
    char* err_buf, int64_t err_buf_len
);

// Free state when done
void <symbol>_free_state(void* state);
```

### Rust cdylib Example

Create a new Rust library with `crate-type = ["cdylib"]`:

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
arrow = { version = "57", features = ["ffi"] }
```

```rust
// src/lib.rs
use arrow::array::{ArrayRef, Int64Array};
use arrow::ffi::{FFI_ArrowArray, FFI_ArrowSchema};
use std::sync::Arc;

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
    // Import input array
    let arg_array = std::ptr::read(*args.offset(0));
    let arg_schema = &**arg_schemas.offset(0);
    let data = arrow::ffi::from_ffi(arg_array, arg_schema).unwrap();
    let array = arrow::array::make_array(data);
    let input = array.as_any().downcast_ref::<Int64Array>().unwrap();

    // Compute result
    let result: Int64Array = input.iter().map(|v| v.map(|x| x * 2)).collect();
    let result_ref: ArrayRef = Arc::new(result);

    // Export output array
    let (ffi_arr, ffi_schema) = arrow::ffi::to_ffi(&result_ref.to_data()).unwrap();
    std::ptr::write(out_array, ffi_arr);
    std::ptr::write(out_schema, ffi_schema);
    0
}
```

Build and use:

```bash
cargo build --release
```

```sql
CREATE FUNCTION acme.double_val(Int64) RETURNS Int64
  WITH (runner = 'lib', logic = './target/release/libmy_funcs.dylib:double_val')
```

### Manifest Function (Bulk Discovery)

For libraries with multiple functions, export a manifest function:

```rust
use std::ffi::{c_char, CString};

const MANIFEST: &str = r#"{"functions": [
    {"name": "double_val", "symbol": "double_val",
     "input_types": ["Int64"], "return_type": "Int64", "kind": "scalar"},
    {"name": "int_sum", "symbol": "int_sum",
     "input_types": ["Int64"], "return_type": "Int64", "kind": "aggregate"}
]}"#;

#[no_mangle]
pub extern "C" fn bundlebase_functions() -> *const c_char {
    CString::new(MANIFEST).unwrap().into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn bundlebase_free_manifest(ptr: *const c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr as *mut c_char));
    }
}
```

Then register all functions at once:

```sql
CREATE FUNCTIONS FROM './target/release/libmy_funcs.dylib'
  WITH (runner = 'lib', namespace = 'acme')
```

## IPC Functions

IPC functions run as separate processes. Bundlebase communicates with them via Arrow IPC over stdin/stdout.

### IPC Discovery Protocol

IPC executables support bulk discovery via the `--bundlebase-functions` flag:

```bash
$ ./my_func --bundlebase-functions
{"functions": [
  {"name": "double_val", "input_types": ["Int64"], "return_type": "Int64", "kind": "scalar"}
]}
```

Register all discovered functions:

```sql
CREATE FUNCTIONS FROM './my_func'
  WITH (runner = 'ipc', namespace = 'tools')
```

## Manifest JSON Format

Both Lib and IPC runners use the same JSON manifest format:

```json
{
  "functions": [
    {
      "name": "double_val",
      "symbol": "double_val",
      "input_types": ["Int64"],
      "return_type": "Int64",
      "kind": "scalar"
    },
    {
      "name": "my_sum",
      "input_types": ["Int64"],
      "return_type": "Int64",
      "kind": "aggregate"
    }
  ]
}
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Function name (used for SQL registration) |
| `symbol` | No | C symbol name (defaults to `name`) |
| `input_types` | Yes | Arrow type names for parameters |
| `return_type` | Yes | Arrow type name for return value |
| `kind` | No | `scalar` (default) or `aggregate` |

### Supported Arrow Types

`Boolean`, `Int8`, `Int16`, `Int32`, `Int64`, `UInt8`, `UInt16`, `UInt32`, `UInt64`, `Float16`, `Float32`, `Float64`, `Utf8`, `LargeUtf8`, `Binary`, `LargeBinary`, `Date32`, `Date64`
