# Template: Add Python Binding

Use this template when exposing existing Rust functionality to Python. This is for wrapping **already implemented** Rust code - if the Rust code doesn't exist yet, use [new-feature.md](new-feature.md) or [add-operation.md](add-operation.md) first.

## When to Use This Template

- Exposing existing Rust method to Python
- Adding Python-friendly convenience methods
- Creating async/sync bridge for new functionality
- Wrapping Rust types for Python access

## Required Reading

Before implementing, read:

1. **[python-bindings.md](../python-bindings.md)** - PyO3 integration patterns
2. **[sync-api.md](../sync-api.md)** - Async/sync bridge architecture
3. **[python-api.md](../python-api.md)** - Python API conventions
4. **The real code**: `rust/bundlebase-python/src/builder.rs` (existing wrappers), `python/src/bundlebase/__init__.py` (registration), `python/src/bundlebase/sync.py` (sync wrappers)

## How the Python Layer Is Structured

1. **PyO3 wrappers** (`rust/bundlebase-python/src/`): `PyBundle` and `PyBundleBuilder` hold an `Arc`-shared Rust bundle/builder. Methods are async — they clone `slf.inner` and run the Rust call inside `pyo3_async_runtimes::tokio::future_into_py`.
2. **Async API + chaining** (`python/src/bundlebase/__init__.py` and `chain.py`): PyO3 methods are registered in the `_original_methods` dict; mutation methods listed in `mutation_methods` are wrapped to return an `OperationChain`, which queues fluent calls and executes them on `await`.
3. **Sync API** (`python/src/bundlebase/sync.py`): `SyncBundle` / `SyncBundleBuilder` expose explicit sync wrappers that dispatch through `_call_original_method()` and the shared event loop manager.

## Critical Constraints

Python bindings MUST follow these rules:

- ✅ **Clone the inner Arc** - clone `slf.inner` before moving into the async future; never borrow `slf` across an await
- ✅ **Register the method** - add to `_original_methods` (and `mutation_methods` if it mutates and returns the builder)
- ✅ **Provide a sync wrapper** - add the method to `SyncBundleBuilder`/`SyncBundle` in `sync.py`
- ✅ **Type hints** - All parameters and return types annotated (including `__init__.pyi` stub)
- ✅ **Docstrings** - Google-style with examples
- ✅ **Map errors** - Convert Rust errors to Python exceptions
- ✅ **Return self for chaining** - mutation methods participate in fluent chaining

## Implementation Checklist

### 1. Identify Rust API

- [ ] Locate the Rust method you're wrapping (often a `BundleBuilderExt` trait method)
- [ ] Understand its signature (parameters, return type)
- [ ] Identify error conditions
- [ ] Note if it mutates state

**Example:**
```rust
// Rust method to wrap (rust/bundlebase-command/src/builder_ext.rs)
async fn filter(&self, query: &str, params: Vec<ScalarValue>)
    -> Result<&Self, BundlebaseError>;
```

### 2. Add PyO3 Wrapper (Rust side)

- [ ] Open `rust/bundlebase-python/src/builder.rs` (or `bundle.rs` for read methods)
- [ ] Find the `#[pymethods]` block for the appropriate type
- [ ] Clone `slf.inner` (Arc) before the async block
- [ ] Convert Rust types to Python types (String, Vec, etc.)
- [ ] Map errors to Python exceptions

**Example (pattern from builder.rs):**
```rust
#[pymethods]
impl PyBundleBuilder {
    fn filter<'py>(
        slf: PyRef<'_, Self>,
        expression: String,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone(); // Arc clone (cheap)
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .filter(&expression, vec![])
                .await
                .map_err(to_py_err)?;
            Ok(())
        })
    }
}
```

### 3. Register in the Async Python Layer

- [ ] Open `python/src/bundlebase/__init__.py`
- [ ] Add the method to the `_original_methods` dict:
  ```python
  _original_methods = {
      ...
      "filter": _PyBundleBuilder.filter,
  }
  ```
- [ ] If it mutates the bundle and returns the builder, add it to `mutation_methods` so it's wrapped to return an `OperationChain` (enables fluent chaining):
  ```python
  mutation_methods = [..., "filter"]
  ```
- [ ] Methods that return a value (e.g. `fetch` returns a count, `query` returns results) are registered in `_original_methods` only — see the comments in `__init__.py`

### 4. Add Sync Wrapper

- [ ] Open `python/src/bundlebase/sync.py`
- [ ] Add a method to `SyncBundleBuilder` (or `SyncBundle` for read methods)
- [ ] Dispatch via `_call_original_method()` + the loop manager
- [ ] Return `self` for chaining

**Example (pattern from sync.py):**
```python
class SyncBundleBuilder(SyncBundle):
    def filter(self, expression: str) -> "SyncBundleBuilder":
        """Filter rows with a SQL SELECT against the bundle.

        Args:
            expression: SQL query (e.g. "SELECT * FROM bundle WHERE age >= 18")

        Returns:
            Self for fluent chaining
        """
        coro = _call_original_method(self._async, "filter", expression)
        self._async = _loop_manager.run_sync(coro)
        return self
```

### 5. Add Type Stub (if needed)

- [ ] Update `python/src/bundlebase/__init__.pyi` so type checkers can validate usage

### 6. Write Tests

- [ ] Write a thin Python E2E test in `python/tests/` — verify the binding works, do NOT re-test Rust logic
- [ ] Test both async and sync versions
- [ ] Test method chaining
- [ ] Run tests: `poetry run pytest python/tests/`

**Example:**
```python
# python/tests/test_e2e.py
import bundlebase
import bundlebase.sync as bbs

@pytest.mark.asyncio
async def test_filter_async():
    """Test filter binding works (async, chained)."""
    c = await bundlebase.create() \
        .attach(bundlebase.test_datafile("userdata.parquet")) \
        .filter("SELECT * FROM bundle WHERE age >= 18")
    df = await c.to_pandas()
    assert all(df["age"] >= 18)

def test_filter_sync():
    """Test filter binding works (sync)."""
    c = bbs.create()
    c.attach(bbs.test_datafile("userdata.parquet")).filter(
        "SELECT * FROM bundle WHERE age >= 18"
    )
    df = c.to_pandas()
    assert all(df["age"] >= 18)
```

### 7. Documentation

- [ ] Docstrings complete with Args, Returns, Raises, Example
- [ ] Type hints on all parameters
- [ ] Consider adding to [python-api.md](../python-api.md) if significant

### 8. Code Review Checklist

- [ ] PyO3 wrapper clones `slf.inner` before the async block
- [ ] Registered in `_original_methods` (and `mutation_methods` if applicable)
- [ ] Sync wrapper added to `sync.py`
- [ ] Type hints complete (including `.pyi` stub)
- [ ] Docstrings complete with examples
- [ ] Errors mapped to Python exceptions with context
- [ ] Tests cover both async and sync
- [ ] Method chaining works

## Common Pitfalls

### 1. Borrowing the PyO3 wrapper across an await

**Wrong:**
```rust
fn filter<'py>(slf: PyRef<'_, Self>, expr: String, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        slf.inner.filter(&expr, vec![]).await?; // ❌ slf borrowed across await
        Ok(())
    })
}
```

**Right:**
```rust
fn filter<'py>(slf: PyRef<'_, Self>, expr: String, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
    let inner = slf.inner.clone(); // ✅ Arc clone, move into future
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        inner.filter(&expr, vec![]).await.map_err(to_py_err)?;
        Ok(())
    })
}
```

### 2. Forgetting registration in `__init__.py`

If the method is not in `_original_methods`, the chain machinery and sync API can't find it. If a mutation method is not in `mutation_methods`, it won't return an `OperationChain` and fluent chaining breaks.

### 3. Not mapping errors with context

**Wrong:**
```rust
inner.filter(&expr, vec![]).await?; // ❌ Generic error type mismatch
```

**Right:**
```rust
inner.filter(&expr, vec![]).await.map_err(to_py_err)?; // ✅ Converted to Python exception
```

### 4. Missing type hints

**Wrong:**
```python
def filter(self, expression):  # ❌ No types
    ...
```

**Right:**
```python
def filter(self, expression: str) -> "SyncBundleBuilder":  # ✅ Full types
    ...
```

### 5. Incomplete docstrings

**Wrong:**
```python
def filter(self, expression: str) -> "SyncBundleBuilder":
    """Filter data."""  # ❌ No Args, Returns, Example
```

**Right:**
```python
def filter(self, expression: str) -> "SyncBundleBuilder":
    """Filter rows with a SQL SELECT against the bundle.

    Args:
        expression: SQL query, e.g. "SELECT * FROM bundle WHERE age >= 18"

    Returns:
        Self for method chaining

    Example:
        >>> c.filter("SELECT * FROM bundle WHERE age >= 18")
    """
```

### 6. Re-testing Rust logic in Python tests

Python tests verify the binding works end-to-end. SQL-parser edge cases, validation rules, etc. belong in Rust tests (see [testing.md](../testing.md)).

## Success Criteria

Binding is complete when:

- ✅ PyO3 wrapper implemented (Arc clone + `future_into_py`)
- ✅ Registered in `_original_methods` / `mutation_methods`
- ✅ Sync wrapper added in `sync.py` with full docstring
- ✅ Type hints complete
- ✅ Tests pass for both async and sync
- ✅ Method chaining works
- ✅ Code review checklist complete

## Related Templates

- [new-feature.md](new-feature.md) - If Rust implementation doesn't exist yet
- [add-operation.md](add-operation.md) - If wrapping an operation

## Related Documentation

- [python-bindings.md](../python-bindings.md) - PyO3 patterns and Arc cloning
- [sync-api.md](../sync-api.md) - How async/sync bridge works
- [python-api.md](../python-api.md) - Python API conventions
