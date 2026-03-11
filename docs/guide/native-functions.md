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
IMPORT FUNCTION acme.double_val FROM 'lib://target/release/libmy_funcs.dylib:double_val'
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
IMPORT FUNCTION acme.* FROM 'lib://./target/release/libmy_funcs.dylib'
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
IMPORT FUNCTION tools.* FROM 'ipc://./my_func'
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

### IPC Scalar Protocol

When Bundlebase invokes a scalar IPC function, the exchange is:

1. **JSON-RPC request** — `invoke` method with the function name
2. **Arrow IPC input** — length-prefixed (4-byte big-endian u32) Arrow IPC stream containing one RecordBatch with one column per argument
3. **Arrow IPC output** — length-prefixed Arrow IPC stream containing a single-column RecordBatch with the result

### IPC Aggregate Protocol

IPC aggregate functions use four JSON-RPC methods. Aggregate state is **opaque and server-side** — only string state IDs cross the wire.

| Step | Method | Params | Arrow IPC | Response |
|------|--------|--------|-----------|----------|
| 1 | `create_state` | `function` | — | `{"state_id": "0"}` |
| 2 | `accumulate` | `function`, `state_id` | Input batch (one column per arg) | `{"ok": true}` (ack) |
| 3 | `merge` | `function`, `state_id1`, `state_id2` | — | `{"state_id": "2"}` |
| 4 | `evaluate` | `function`, `state_id` | — | `{"ok": true}` (ack), then Arrow IPC output (single-row, single-column) |

**Lifecycle:**

1. `create_state` — allocates a fresh accumulator on the server, returns an opaque state ID.
2. `accumulate` — called once per batch. After the JSON-RPC ack, Bundlebase sends a length-prefixed Arrow IPC stream with the batch data. The server updates its internal state; nothing is returned.
3. `merge` — combines two states (used during parallel/partitioned execution). Returns the merged state ID.
4. `evaluate` — finalizes the aggregate. After the JSON-RPC ack, the server sends a length-prefixed Arrow IPC stream containing a single-row, single-column RecordBatch with the result.

To declare an aggregate function in the manifest, set `"kind": "aggregate"`:

```json
{"name": "my_avg", "input_types": ["Float64"], "return_type": "Float64", "kind": "aggregate"}
```

## Python SDK Functions

The Python SDK provides a `Function` base class (`bundlebase_sdk.function`) for writing IPC function providers. Call `serve_function(instance)` to start the JSON-RPC serve loop.

### Scalar Example

```python
from bundlebase_sdk.function import Function
from bundlebase_sdk.function_serve import serve_function
import pyarrow as pa

class MyFunctions(Function):
    def functions(self):
        return [
            {"name": "double_val", "input_types": ["Int64"], "return_type": "Int64", "kind": "scalar"},
        ]

    def invoke(self, name, batch):
        if name == "double_val":
            col = batch.column(0)
            result = pa.compute.multiply(col, 2)
            return pa.record_batch({"result": result})

if __name__ == "__main__":
    serve_function(MyFunctions())
```

Register and use:

```sql
IMPORT FUNCTION tools.* FROM 'ipc://python:my_functions.py'
SELECT tools.double_val(id) FROM bundle
```

### Aggregate Example

For aggregate functions, implement four additional methods: `create_state`, `accumulate`, `merge`, and `evaluate`. The SDK's `_AggregateStateStore` manages state lifecycle automatically — your methods just work with plain Python objects.

```python
from bundlebase_sdk.function import Function
from bundlebase_sdk.function_serve import serve_function
import pyarrow as pa

class MyFunctions(Function):
    def functions(self):
        return [
            {"name": "my_avg", "input_types": ["Float64"], "return_type": "Float64", "kind": "aggregate"},
        ]

    def invoke(self, name, batch):
        raise NotImplementedError("No scalar functions")

    def create_state(self, name):
        # Return any Python object — it stays server-side
        return {"sum": 0.0, "count": 0}

    def accumulate(self, name, state, batch):
        col = batch.column(0)
        state["sum"] += pa.compute.sum(col).as_py()
        state["count"] += len(col)
        return state

    def merge(self, name, state1, state2):
        return {
            "sum": state1["sum"] + state2["sum"],
            "count": state1["count"] + state2["count"],
        }

    def evaluate(self, name, state):
        if state["count"] == 0:
            return pa.scalar(None, type=pa.float64())
        return pa.scalar(state["sum"] / state["count"], type=pa.float64())

if __name__ == "__main__":
    serve_function(MyFunctions())
```

Register and use:

```sql
IMPORT FUNCTION stats.* FROM 'ipc://python:my_functions.py'
SELECT category, stats.my_avg(amount) FROM bundle GROUP BY category
```

**Key points:**
- State objects are arbitrary Python objects — they never cross the wire
- `accumulate` receives a `pa.RecordBatch` with one column per function argument
- `evaluate` must return a `pa.Scalar` (or a plain Python value that PyArrow can convert)
- After `evaluate`, the state is automatically cleaned up

### Supported Arrow Types

`Boolean`, `Int8`, `Int16`, `Int32`, `Int64`, `UInt8`, `UInt16`, `UInt32`, `UInt64`, `Float16`, `Float32`, `Float64`, `Utf8`, `LargeUtf8`, `Binary`, `LargeBinary`, `Date32`, `Date64`
