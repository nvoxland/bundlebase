# Python Bindings Architecture

## PyO3 Integration

The Python bindings (`src/python/`) use PyO3 to expose Rust functionality with two main classes:

```rust
#[pymodule(name = "_Bundlebase")]
fn Bundlebase(m: &Bound<'_, PyModule>) -> PyResult<()> {
    #[pyclass]
    struct PyBundlebase {
        inner: Bundlebase,  // Wraps read-only committed container
    }

    #[pyclass]
    struct PyBundlebaseBuilder {
        inner: BundlebaseBuilder,  // Wraps mutable extending container
    }

    m.add_function(wrap_pyfunction!(create, m)?)?;    // Create new mutable container
    m.add_function(wrap_pyfunction!(open, m)?)?;      // Open committed container
    m.add_class::<PyBundlebase>()?;
    m.add_class::<PyBundlebaseBuilder>()?;
    Ok(())
}
```

**Class separation:**
- **PyBundlebase**: Wraps Bundlebase - exposes read-only operations (query, to_pandas, to_dict, schema, num_rows, etc.)
- **PyBundlebaseBuilder**: Wraps BundlebaseBuilder - exposes mutable operations (attach, filter, remove_column, commit, etc.) and delegates read operations

**Module organization:**
- `python/mod.rs` - Module entry point, pymodule macro
- `python/bundle.rs` - PyBundlebase implementation
- `python/builder.rs` - PyBundlebaseBuilder implementation
- `python/schema.rs` - PySchema and PySchemaField bindings
- `python/function_impl.rs` - Python function bridge (PythonFunctionImpl)
- `python/utils.rs` - Shared Python binding utilities

## Async Bridge: Tokio ↔ Asyncio

All async methods use `pyo3_async_runtimes::tokio::future_into_py` to bridge Rust async (Tokio) with Python async (asyncio):

```rust
fn attach<'py>(
    mut slf: PyRefMut<'_, Self>,
    url: String,
    py: Python<'py>
) -> PyResult<Bound<'py, PyAny>> {
    // Mutable reference for in-place mutation
    let slf_inner = &mut slf.inner;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        slf_inner.attach(url.as_str()).await.map_err(|e| PyException::new_err(e.to_string()))?;
        Python::with_gil(|_py| {
            // Container mutated in place, return updated self
            Py::new(_py, PyBundlebaseBuilder { inner: slf_inner.clone() })
        })
    })
}
```

**How it works:**
1. Take mutable reference to PyBundlebaseBuilder
2. Wrap Rust async operation in `future_into_py`
3. Operation mutates container in-place
4. Error handling converts Rust errors to Python exceptions
5. Result bridges back to Python's asyncio event loop
6. Return updated PyBundlebaseBuilder

**Pattern changes from immutable to mutable:**
- Old: Created new container, returned new PyBundlebase
- New: Mutates in place, returns same container after mutation
- Python sees: `await c.attach(...)` mutates `c` and returns it

## Lifetime Management

Mutable methods use `PyRefMut` for in-place mutation:

```rust
fn attach<'py>(
    mut slf: PyRefMut<'_, Self>,  // Mutable reference to Python object
    url: String,
    py: Python<'py>               // Python GIL token with lifetime 'py
) -> PyResult<Bound<'py, PyAny>>  // Return type bound to same lifetime
```

**Why?**
- `PyRefMut` allows mutation of Python object from Rust
- Returned value is still bound to GIL lifetime
- Prevents dangling references
- Type safety across the Rust/Python boundary
- Rust borrow checker enforces proper mutation semantics

## Python Function Integration

**PythonFunctionImpl** (`src/python/function_impl.rs`) bridges Python callables to Rust:

```rust
impl FunctionImpl for PythonFunctionImpl {
    fn execute(&self, sig: Arc<FunctionSig>) -> Result<Arc<dyn DataGenerator>> {
        Ok(Arc::new(PythonDataGenerator {
            py_fn: self.py_fn.clone(),  // Python function reference
            schema: sig.schema().clone(),
        }))
    }
}
```

**PythonDataGenerator** calls the Python function with page numbers:

```rust
impl DataGenerator for PythonDataGenerator {
    fn next(&self, page: usize) -> Result<Option<RecordBatch>> {
        Python::with_gil(|py| {
            // Call Python: function(page=page, schema=schema)
            let result = self.py_fn.call(py, (), Some(kwargs))?;

            if result.is_none(py) {
                return Ok(None);  // Python returned None - no more data
            }

            // Convert PyArrow RecordBatch to Rust RecordBatch
            RecordBatch::from_pyarrow_bound(&result.into_bound(py))
        })
    }
}
```

**Python side:**
```python
def my_data(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
    if page == 0:
        return pa.record_batch(data={"id": [1, 2, 3], "name": ["a", "b", "c"]}, schema=schema)
    return None  # No more pages
```

## Error Handling

**Current implementation:**
- Python bindings use `.map_err()` to convert Rust errors to Python exceptions
- Descriptive error messages passed to Python
- Python sees proper exceptions with context

**Example error handling:**
```rust
slf_inner.attach(url.as_str())
    .await
    .map_err(|e| PyException::new_err(e.to_string()))?
```

**Result in Python:**
```python
try:
    await c.attach("nonexistent.parquet")
except Exception as e:
    print(f"Error: {e}")  # Detailed error message from Rust
```

**Improvements made:**
- Replaced `.unwrap()` with proper `.map_err()` propagation
- Rust errors converted to Python exceptions with messages
- Errors surface with context instead of panics
- Better debugging experience for Python users
