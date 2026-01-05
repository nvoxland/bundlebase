# Template: Add New Feature

Use this template when adding a completely new feature to bundlebase that doesn't fit into the existing operation pipeline.

## Examples of Features

- New data source adapter (JSON, Avro, etc.)
- New query optimization pass
- New indexing strategy
- New Python API surface area
- New storage backend

## Required Reading

Before implementing, read:

1. **[architecture.md](../architecture.md)** - Understand three-tier architecture
2. **[ai-rules.md](../ai-rules.md)** - Critical constraints (streaming, no unwrap, etc.)
3. **[anti-patterns.md](../anti-patterns.md)** - What NOT to do
4. **Relevant ADRs** in [decisions/](../decisions/) - Especially ADR-002 (DataFusion), ADR-003 (Streaming)

## Critical Constraints

Must follow these rules:

- ✅ **Streaming execution only** - Use `execute_stream()`, never `collect()`
- ✅ **No `.unwrap()`** - Code will not compile if you use it
- ✅ **Proper error handling** - Use `Result<T>` and `?` operator
- ✅ **Async where needed** - I/O operations must be async
- ✅ **Type safety** - Leverage Rust's type system
- ✅ **No `mod.rs` files** - Use named module files (e.g., `feature.rs`)

See [ai-rules.md](../ai-rules.md) for full list.

## Implementation Checklist

### 1. Planning Phase

- [ ] Read all required documentation files
- [ ] Understand how feature fits into architecture
- [ ] Identify which tier(s) the feature touches (Bundle trait, Bundle, BundleBuilder)
- [ ] Check if feature requires new dependencies
- [ ] Sketch out API design (Rust and Python)
- [ ] Identify potential performance impacts

### 2. Rust Implementation

- [ ] Create new module file (e.g., `src/new_feature.rs`, NOT `src/new_feature/mod.rs`)
- [ ] Define Rust types and traits
- [ ] Implement core logic with streaming execution
- [ ] Add proper error handling (no `.unwrap()`)
- [ ] Add logging at appropriate levels (see [logging.md](../logging.md))
- [ ] Write Rust unit tests
- [ ] Run `cargo clippy` and fix all warnings
- [ ] Run `cargo test` and verify all tests pass

### 3. Python Bindings (if applicable)

- [ ] Read [python-bindings.md](../python-bindings.md)
- [ ] Create PyO3 wrapper in `python/bundlebase/src/`
- [ ] Handle async/sync bridge if needed (see [sync-api.md](../sync-api.md))
- [ ] Map Rust errors to Python exceptions
- [ ] Add type hints (use `typing` module)
- [ ] Clone `Arc<T>` wrappers for Python return values

### 4. Documentation

- [ ] Add docstrings to all public Rust items (`///` comments)
- [ ] Add Python docstrings with examples
- [ ] Update [CLAUDE.md](../../CLAUDE.md) if feature changes development workflow
- [ ] Consider creating new ADR in [decisions/](../decisions/) if architectural
- [ ] Update [README.md](../README.md) navigation if new major section

### 5. Testing

- [ ] Write Rust unit tests (`#[test]` functions)
- [ ] Write Rust integration tests (if needed, in `tests/` directory)
- [ ] Write Python E2E tests in `python/tests/` (see [testing.md](../testing.md))
- [ ] Test with sample data files
- [ ] Test error conditions (invalid input, missing files, etc.)
- [ ] Run full test suite: `poetry run pytest`
- [ ] Verify streaming behavior (check memory usage doesn't grow with dataset size)

### 6. Performance Validation

- [ ] Verify streaming execution is used (no `collect()` calls)
- [ ] Test with dataset larger than RAM
- [ ] Profile memory usage (should be constant, not proportional to data size)
- [ ] Check for unnecessary clones or allocations
- [ ] Run `cargo build --release` and test release performance

### 7. Code Review Checklist

- [ ] No `.unwrap()` or `.expect()` (except in tests)
- [ ] No `collect()` calls on DataFrames
- [ ] All public items have documentation
- [ ] Error messages are descriptive and include context
- [ ] Follows naming conventions (snake_case for functions, PascalCase for types)
- [ ] No `mod.rs` files
- [ ] Python bindings return cloned Arc wrappers
- [ ] Tests cover success and error paths

## Common Pitfalls

### 1. Using `collect()` instead of streaming

**Wrong:**
```rust
let data = df.collect().await?; // ❌ Loads entire dataset into memory
```

**Right:**
```rust
let stream = df.execute_stream().await?; // ✅ Streaming execution
while let Some(batch) = stream.next().await {
    // Process batch incrementally
}
```

### 2. Using `.unwrap()` for error handling

**Wrong:**
```rust
let value = option.unwrap(); // ❌ Will not compile
```

**Right:**
```rust
let value = option.ok_or_else(|| BundlebaseError::from("value not found"))?; // ✅
```

### 3. Creating `mod.rs` files

**Wrong:**
```
src/
└── feature/
    └── mod.rs  ❌
```

**Right:**
```
src/
└── feature.rs  ✅
```

### 4. Forgetting async/sync bridge in Python

**Wrong:**
```python
# Python binding exposes async only
async def new_feature(self):  # ❌ Hard to use in scripts
    ...
```

**Right:**
```python
# Provide both async and sync wrappers
async def new_feature(self):  # ✅ For async contexts
    ...

class Container:  # Sync wrapper
    def new_feature(self):  # ✅ For scripts/Jupyter
        return sync(self._async_impl.new_feature())
```

### 5. Not testing with large datasets

**Wrong:**
```python
# Test with tiny dataset
def test_feature():
    c = bundlebase.create()
    c.attach("10_row_file.parquet")  # ❌ Doesn't test streaming
```

**Right:**
```python
# Test with dataset larger than RAM
def test_feature_streaming():
    c = bundlebase.create()
    c.attach("10gb_file.parquet")  # ✅ Verifies streaming works
    result = c.to_pandas()  # Should use constant memory
```

## Example: Adding JSON Adapter

Here's a reference implementation following this template:

### 1. Planning
- Feature: Support JSON files as data source
- Architecture: Extends adapter system (Bundle trait implementation)
- Dependencies: May need `serde_json` crate
- Python API: `c.attach("data.json")`

### 2. Rust Implementation
```rust
// src/io/adapters/json.rs
use datafusion::prelude::*;
use crate::error::Result;

pub struct JsonAdapter {
    path: String,
}

impl JsonAdapter {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    pub async fn to_dataframe(&self, ctx: &SessionContext) -> Result<DataFrame> {
        // Use DataFusion's JSON reader (automatically streaming)
        let df = ctx.read_json(&self.path, Default::default())
            .await
            .map_err(|e| format!("Failed to read JSON {}: {}", self.path, e))?;
        Ok(df)
    }
}
```

### 3. Python Binding
```python
# python/bundlebase/src/container.py
def attach(self, path: str) -> "Container":
    """Attach a data file (Parquet, JSON, CSV, etc.)."""
    if path.endswith(".json"):
        self._inner.attach_json(path)
    # ... existing logic
    return self
```

### 4. Testing
```python
# python/tests/test_json_adapter.py
def test_json_adapter():
    c = bundlebase.create()
    c.attach("test_data.json")
    df = c.to_pandas()
    assert len(df) > 0
```

## Success Criteria

Feature is complete when:

- ✅ Rust code compiles with no warnings (`cargo clippy`)
- ✅ All Rust tests pass (`cargo test`)
- ✅ Python bindings work correctly
- ✅ All Python E2E tests pass (`poetry run pytest`)
- ✅ Documentation is complete (Rust docs, Python docstrings)
- ✅ Streaming execution verified (constant memory usage)
- ✅ No critical constraints violated
- ✅ Code review checklist complete

## Related Templates

- [add-operation.md](add-operation.md) - If feature is a transformation operation
- [add-python-binding.md](add-python-binding.md) - If only adding Python API
- [performance-review.md](performance-review.md) - For performance-critical features

## Related Documentation

- [architecture.md](../architecture.md) - How features fit into architecture
- [decisions/003-streaming-only.md](../decisions/003-streaming-only.md) - Why streaming matters
- [testing.md](../testing.md) - Testing strategy
