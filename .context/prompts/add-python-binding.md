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
4. **[decisions/005-mutable-operations.md](../decisions/005-mutable-operations.md)** - Why methods return `self`

## Critical Constraints

Python bindings MUST follow these rules:

- ✅ **Clone Arc wrappers** - Python methods return `self.clone()`, not `&mut self`
- ✅ **Provide both async and sync** - AsyncContainer AND Container classes
- ✅ **Type hints** - All parameters and return types annotated
- ✅ **Docstrings** - Google-style with examples
- ✅ **Map errors** - Convert Rust errors to Python exceptions
- ✅ **Return self for chaining** - Enable fluent API

## Implementation Checklist

### 1. Identify Rust API

- [ ] Locate the Rust method you're wrapping
- [ ] Understand its signature (parameters, return type)
- [ ] Check if it's async or sync
- [ ] Identify error conditions
- [ ] Note if it mutates state

**Example:**
```rust
// Rust method to wrap
impl BundleBuilder {
    pub fn filter(&mut self, expr: &str) -> Result<&mut Self> {
        // ...
    }
}
```

### 2. Add PyO3 Wrapper (Rust side)

- [ ] Open `python/bundlebase/src/container.rs`
- [ ] Find the `#[pymethods]` block for the appropriate type
- [ ] Add new method with `#[pyo3(name = "method_name")]` if needed
- [ ] Convert Rust types to Python types (String, Vec, etc.)
- [ ] Map errors using `map_err(|e| PyErr::new::<PyRuntimeError, _>())`
- [ ] Return cloned wrapper for Python

**Example:**
```rust
#[pymethods]
impl PyBundleBuilder {
    fn filter(&mut self, expression: String) -> PyResult<Self> {
        // Call Rust method
        self.inner.filter(&expression)
            .map_err(|e| PyErr::new::<PyRuntimeError, _>(
                format!("Filter failed: {}", e)
            ))?;

        // Clone for Python (Arc-based, cheap)
        Ok(self.clone())
    }
}
```

### 3. Add Async Python Method

- [ ] Open `python/bundlebase/container.py`
- [ ] Add method to `AsyncContainer` class
- [ ] Add type hints for all parameters and return type
- [ ] Add Google-style docstring with example
- [ ] Call the PyO3 method via `await self._inner.method()`
- [ ] Return `self` for chaining

**Example:**
```python
class AsyncContainer:
    async def filter(self, expression: str) -> "AsyncContainer":
        """Filter rows based on SQL WHERE clause.

        Args:
            expression: SQL WHERE expression (without the WHERE keyword)

        Returns:
            Self for method chaining

        Raises:
            RuntimeError: If expression is invalid or columns don't exist

        Example:
            >>> c = await bundlebase.create()
            >>> await c.attach("data.parquet")
            >>> await c.filter("age >= 18 AND status = 'active'")
            >>> df = await c.to_pandas()
        """
        await self._inner.filter(expression)
        return self
```

### 4. Add Sync Python Method

- [ ] Open `python/bundlebase/container_sync.py`
- [ ] Add method to `Container` class
- [ ] Use `sync()` helper to wrap async call
- [ ] Same signature and docstring as async version (but sync)
- [ ] Return `self` for chaining

**Example:**
```python
class Container:
    def filter(self, expression: str) -> "Container":
        """Filter rows based on SQL WHERE clause.

        Args:
            expression: SQL WHERE expression (without the WHERE keyword)

        Returns:
            Self for method chaining

        Raises:
            RuntimeError: If expression is invalid or columns don't exist

        Example:
            >>> c = bundlebase.create()
            >>> c.attach("data.parquet")
            >>> c.filter("age >= 18 AND status = 'active'")
            >>> df = c.to_pandas()
        """
        sync(self._async_impl.filter(expression))
        return self
```

### 5. Add Type Stub (if needed)

- [ ] If using complex types, add to `python/bundlebase/container.pyi`
- [ ] Ensure type checkers can validate usage

### 6. Write Tests

- [ ] Write Python E2E test in `python/tests/`
- [ ] Test both async and sync versions
- [ ] Test success case
- [ ] Test error cases
- [ ] Test method chaining
- [ ] Run tests: `poetry run pytest`

**Example:**
```python
# python/tests/test_filter.py

def test_filter_sync():
    """Test filter with synchronous API."""
    c = bundlebase.create()
    c.attach("tests/data/sample.parquet")
    c.filter("age >= 18")
    df = c.to_pandas()

    assert all(df["age"] >= 18)
    assert len(df) > 0

@pytest.mark.asyncio
async def test_filter_async():
    """Test filter with async API."""
    c = await bundlebase.create_async()
    await c.attach("tests/data/sample.parquet")
    await c.filter("age >= 18")
    df = await c.to_pandas()

    assert all(df["age"] >= 18)

def test_filter_chaining():
    """Test method chaining."""
    c = bundlebase.create()
    c.attach("tests/data/sample.parquet") \
        .filter("age >= 18") \
        .filter("age < 65")

    df = c.to_pandas()
    assert all((df["age"] >= 18) & (df["age"] < 65))

def test_filter_error():
    """Test error handling."""
    c = bundlebase.create()
    c.attach("tests/data/sample.parquet")

    with pytest.raises(RuntimeError, match="Column not found"):
        c.filter("invalid_column > 0")
```

### 7. Documentation

- [ ] Docstrings complete with Args, Returns, Raises, Example
- [ ] Type hints on all parameters
- [ ] Consider adding to [python-api.md](../python-api.md) if significant
- [ ] Update examples in documentation if needed

### 8. Code Review Checklist

- [ ] PyO3 wrapper returns cloned `Self`
- [ ] Async Python method exists
- [ ] Sync Python method exists
- [ ] Both methods return `self` for chaining
- [ ] Type hints complete
- [ ] Docstrings complete with examples
- [ ] Errors mapped to Python exceptions with context
- [ ] Tests cover both async and sync
- [ ] Tests cover error cases
- [ ] Method chaining works

## Common Pitfalls

### 1. Returning Rust reference instead of clone

**Wrong:**
```rust
#[pymethods]
impl PyBundleBuilder {
    fn filter(&mut self, expr: String) -> PyResult<&mut Self> {
        // ❌ Python can't hold Rust references
        self.inner.filter(&expr)?;
        Ok(self)
    }
}
```

**Right:**
```rust
#[pymethods]
impl PyBundleBuilder {
    fn filter(&mut self, expr: String) -> PyResult<Self> {
        // ✅ Clone for Python (Arc-based, cheap)
        self.inner.filter(&expr)?;
        Ok(self.clone())
    }
}
```

### 2. Only providing async API

**Wrong:**
```python
# Only async version
class AsyncContainer:
    async def filter(self, expr: str) -> "AsyncContainer":
        ...

# ❌ No sync version - hard to use in scripts/Jupyter
```

**Right:**
```python
# Both async and sync
class AsyncContainer:
    async def filter(self, expr: str) -> "AsyncContainer":
        ...

class Container:
    def filter(self, expr: str) -> "Container":
        sync(self._async_impl.filter(expr))  # ✅ Sync wrapper
        return self
```

### 3. Not mapping errors with context

**Wrong:**
```rust
self.inner.filter(&expr)?; // ❌ Generic error message
```

**Right:**
```rust
self.inner.filter(&expr)
    .map_err(|e| PyErr::new::<PyRuntimeError, _>(
        format!("Filter failed: {}", e)  // ✅ Context added
    ))?;
```

### 4. Missing type hints

**Wrong:**
```python
def filter(self, expression):  # ❌ No types
    ...
```

**Right:**
```python
def filter(self, expression: str) -> "Container":  # ✅ Full types
    ...
```

### 5. Incomplete docstrings

**Wrong:**
```python
def filter(self, expression: str) -> "Container":
    """Filter data."""  # ❌ No Args, Returns, Example
```

**Right:**
```python
def filter(self, expression: str) -> "Container":
    """Filter rows based on SQL WHERE clause.

    Args:
        expression: SQL WHERE expression

    Returns:
        Self for method chaining

    Raises:
        RuntimeError: If expression is invalid

    Example:
        >>> c.filter("age >= 18")
    """
```

### 6. Not returning self for chaining

**Wrong:**
```python
def filter(self, expr: str) -> None:
    sync(self._async_impl.filter(expr))
    # ❌ Can't chain
```

**Right:**
```python
def filter(self, expr: str) -> "Container":
    sync(self._async_impl.filter(expr))
    return self  # ✅ Enables chaining
```

## Example: Wrapping `remove_column()`

Complete example of wrapping a Rust method:

### 1. Rust PyO3 Wrapper

```rust
// python/bundlebase/src/container.rs

#[pymethods]
impl PyBundleBuilder {
    fn remove_column(&mut self, name: String) -> PyResult<Self> {
        self.inner.remove_column(&name)
            .map_err(|e| PyErr::new::<PyRuntimeError, _>(
                format!("Failed to remove column '{}': {}", name, e)
            ))?;
        Ok(self.clone())
    }
}
```

### 2. Async Python Method

```python
# python/bundlebase/container.py

class AsyncContainer:
    async def remove_column(self, name: str) -> "AsyncContainer":
        """Remove a column from the dataset.

        Args:
            name: Name of the column to remove

        Returns:
            Self for method chaining

        Raises:
            RuntimeError: If column doesn't exist

        Example:
            >>> c = await bundlebase.create_async()
            >>> await c.attach("data.parquet")
            >>> await c.remove_column("ssn")  # Remove sensitive column
            >>> df = await c.to_pandas()
        """
        await self._inner.remove_column(name)
        return self
```

### 3. Sync Python Method

```python
# python/bundlebase/container_sync.py

class Container:
    def remove_column(self, name: str) -> "Container":
        """Remove a column from the dataset.

        Args:
            name: Name of the column to remove

        Returns:
            Self for method chaining

        Raises:
            RuntimeError: If column doesn't exist

        Example:
            >>> c = bundlebase.create()
            >>> c.attach("data.parquet")
            >>> c.remove_column("ssn")  # Remove sensitive column
            >>> df = c.to_pandas()
        """
        sync(self._async_impl.remove_column(name))
        return self
```

### 4. Tests

```python
# python/tests/test_remove_column.py

def test_remove_column():
    c = bundlebase.create()
    c.attach("tests/data/sample.parquet")
    c.remove_column("age")

    df = c.to_pandas()
    assert "age" not in df.columns

def test_remove_column_error():
    c = bundlebase.create()
    c.attach("tests/data/sample.parquet")

    with pytest.raises(RuntimeError, match="Column.*not found"):
        c.remove_column("nonexistent")
```

## Success Criteria

Binding is complete when:

- ✅ PyO3 wrapper implemented and returns cloned `Self`
- ✅ Async Python method added with full docstring
- ✅ Sync Python method added with sync() wrapper
- ✅ Type hints complete
- ✅ Tests pass for both async and sync
- ✅ Error cases tested
- ✅ Method chaining works
- ✅ Code review checklist complete

## Related Templates

- [new-feature.md](new-feature.md) - If Rust implementation doesn't exist yet
- [add-operation.md](add-operation.md) - If wrapping an operation

## Related Documentation

- [python-bindings.md](../python-bindings.md) - PyO3 patterns and Arc cloning
- [sync-api.md](../sync-api.md) - How async/sync bridge works
- [python-api.md](../python-api.md) - Python API conventions
