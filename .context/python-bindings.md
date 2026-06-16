# Python Bindings Architecture

## PyO3 Integration

The Python bindings live in the `rust/bundlebase-python/` crate and use PyO3 to expose Rust functionality. The compiled extension module is named `_bundlebase` and is wrapped by the pure-Python package in `python/src/bundlebase/`:

```rust
// rust/bundlebase-python/src/lib.rs
#[pymodule(name = "_bundlebase")]
fn bundlebase(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(create, m)?)?;    // async: create new mutable bundle
    m.add_function(wrap_pyfunction!(open, m)?)?;      // async: open committed bundle
    m.add_function(wrap_pyfunction!(upgrade_bundle, m)?)?;
    m.add_function(wrap_pyfunction!(test_datafile, m)?)?;
    m.add_function(wrap_pyfunction!(random_memory_url, m)?)?;
    m.add_class::<PyBundle>()?;
    m.add_class::<PyBundleBuilder>()?;
    m.add_class::<PySchema>()?;
    m.add_class::<PyRecordBatchStream>()?;
    // ... PyChange, PyBundleStatus, PyCommit, PyOperation, PySessionContext, etc.
    Ok(())
}
```

**Class separation:**
- **PyBundle** (`bundle.rs`): Wraps `Arc<Bundle>` - read-only operations (query, as_pyarrow, as_pyarrow_stream, schema, num_rows, explain, view/views, extend, describe_data, etc.)
- **PyBundleBuilder** (`builder.rs`): Wraps `Arc<BundleBuilder>` - mutable operations (attach, filter, drop_column, commit, create_view, create_index, fetch, etc.) plus the read operations

**Module organization** (`rust/bundlebase-python/src/`):
- `lib.rs` - Module entry point, `#[pymodule]` macro, bridge registration
- `bundle.rs` - PyBundle implementation
- `builder.rs` - PyBundleBuilder, PyChange, PyBundleStatus, PyFetchResults
- `schema.rs` - PySchema and PySchemaField bindings
- `record_batch_stream.rs` - PyRecordBatchStream (streaming query results)
- `bundle_config.rs` - PyBundleConfig and config dict conversion
- `commit.rs` / `operation.rs` - PyCommit / PyOperation history objects
- `session_context.rs` - PySessionContext
- `python_function.rs` - PyFunctionBridge (in-process Python SQL functions)
- `native_source.rs` - PyNativeBridge (in-process Python connectors)
- `progress.rs` - Progress tracking functions
- `utils.rs` - Shared utilities (e.g., `convert_py_params`)

**Python wrapper layer** (`python/src/bundlebase/`):
- `__init__.py` re-exports `_bundlebase` types, registers original methods, and wraps mutation methods so they return `OperationChain` (see `chain.py`); wraps `query()` to return `QueryResult` (see `conversion.py`)
- `sync.py` provides the synchronous wrapper API (see [sync-api.md](sync-api.md))

## Async Bridge: Tokio ↔ Asyncio

All async methods use `pyo3_async_runtimes::tokio::future_into_py` to bridge Rust async (Tokio) with Python async (asyncio). The key pattern: the pyclass holds an `Arc` of the core type, and each method **clones the Arc into the future** (the future cannot borrow from `slf`):

```rust
// rust/bundlebase-python/src/builder.rs (simplified, real code)
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PyBundleBuilder {
    inner: Arc<BundleBuilder>,
}

#[pymethods]
impl PyBundleBuilder {
    #[pyo3(signature = (location, pack="base"))]
    fn attach<'py>(
        slf: PyRef<'_, Self>,
        location: &str,
        pack: &str,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone();          // clone Arc, no borrow held across await
        let location = location.to_string();    // own all arguments
        let pack = if pack == "base" { None } else { Some(pack.to_string()) };
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .attach(location.as_str(), pack.as_deref())
                .await
                .map_err(|e| to_py_error_ctx("Failed to attach", e))?;
            // Re-attach to Python to construct the return value
            Python::attach(|py| Py::new(py, PyBundleBuilder { inner }).map_err(|e| to_py_error(e)))
        })
    }
}
```

**How it works:**
1. Take `PyRef` (shared borrow) of the pyclass and clone its `Arc<BundleBuilder>`
2. Convert all borrowed arguments to owned values (`to_string()`)
3. Wrap the Rust async operation in `future_into_py` - the `async move` block owns everything it needs
4. The core `BundleBuilder` uses interior mutability, so `&self` methods mutate the bundle in place
5. Errors are converted to Python exceptions via `to_py_error` / `to_py_error_ctx`
6. On success, `Python::attach` re-acquires the GIL to build the returned `PyBundleBuilder`, which shares the same `Arc` (same underlying bundle)

**Python sees:** `await c.attach(...)` mutates the bundle behind `c` and resolves to a builder wrapping the same bundle.

**Why Arc-clone instead of `&mut slf.inner`?** The future returned by `future_into_py` must be `'static` - it outlives the method call, so it cannot hold a borrow of the Python object. Cloning the `Arc` gives the future shared ownership of the underlying `BundleBuilder` instead.

## Error Handling

Helper functions in `builder.rs` convert Rust errors to Python exceptions:

```rust
/// Helper function to create a PyErr with operation context
fn to_py_error<E: std::fmt::Display>(err: E) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(err.to_string())
}

/// Helper function to create a PyErr with operation context description
fn to_py_error_ctx<E: std::fmt::Display>(context: &str, err: E) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("{}: {}", context, err))
}
```

**Current behavior:** nearly all errors surface in Python as `ValueError` with a descriptive message. Stream execution and batch-iteration failures (e.g., in `PyRecordBatchStream.next_batch()`, `as_pyarrow()` collection) raise `RuntimeError`. There is no per-error-kind mapping to other exception types. See [errors.md](errors.md#python-error-mapping) for details.

**Result in Python:**
```python
try:
    await c.attach("nonexistent.parquet")
except ValueError as e:
    print(f"Error: {e}")  # "Failed to attach: <detailed message from Rust>"
```

## Python Function Integration

In-process Python SQL functions and connectors are wired up through bridge traits registered at module init (`lib.rs`):

- **`PyFunctionBridge`** (`python_function.rs`) implements the core `PythonFunctionBridge` trait. It loads `python::module:callable` entrypoints, converts Arrow scalars to/from PyArrow, and supports scalar invocation plus aggregate state functions (`invoke`, `aggregate_create_state`, `aggregate_accumulate`, `aggregate_merge`, `aggregate_evaluate`). Function metadata (input/return types, kind) is auto-detected by calling the module's `bundlebase_metadata()` function.
- **`PyNativeBridge`** (`native_source.rs`) implements the core `NativePythonBridge` trait for in-process Python connectors (`discover`, `data`, `stable_url`), converting JSON args to kwargs and PyArrow results back to Arrow.

Both bridges call into Python with `Python::attach` (GIL acquisition) from Rust threads.

**Python side (temp function example):**
```python
# my_module.py
def bundlebase_metadata():
    return {"functions": [
        {"name": "double_val", "kind": "scalar",
         "input_types": ["Int64"], "return_type": "Int64"},
    ]}

def double_val(x):
    return x * 2
```

```python
await c.import_temp_function("acme.double_val", "python::my_module:double_val")
# Use in SQL: SELECT acme.double_val(id) FROM bundle
```

Python-runtime functions/connectors are session-only (`import_temp_*`); persistent imports (`import_function` / `import_connector`) require a serializable runtime (ipc, ffi, etc.).
