# Testing Strategy

## Where Tests Live

**Most test coverage is in Rust.** Rust integration tests under
`rust/bundlebase/tests/` are the primary place to add e2e tests for
operations, query paths, sources, sidecars, version validation, views,
joins, updates, and anything else that exercises the bundlebase core.

Python tests are deliberately thin: they exist to verify the Python binding
layer works end-to-end. They do NOT re-test underlying Rust business logic.
If you find yourself wanting to add a Python test to check operation
correctness, add a Rust test instead.

## Test Coverage Overview

**Rust Lib Tests** (`src/`):
- Module unit tests, data storage tests, function registry tests, schema
  tracking, versioning, row indexing.

**Rust Integration Tests** (`rust/bundlebase/tests/`):
- **Primary home for e2e coverage.** Split by feature area —
  `basic_e2e`, `source_e2e`, `update_e2e`, `version_validation_e2e`,
  `views_e2e`, `index_e2e`, `query_e2e`, `bundle_schema_e2e`, etc.
- Cover all operations (attach, remove, rename, filter, select, join,
  commit/open, functions, metadata, row indexing, sources, sidecars,
  version checks, narrow-projection bypass, etc.).
- Shared helpers live in `tests/common/mod.rs`; call `common::init_catalog()`
  from each test's `fn init()` rather than duplicating the `Once` boilerplate.

**Python E2E Tests** (`python/tests/test_e2e.py`):
- **Scope**: High-level smoke checks that the Python binding exposes the
  expected API — file formats (Parquet/CSV/JSON), conversions (pandas,
  polars, dict, numpy), custom functions, metadata, schema introspection,
  commit/open roundtrip.
- **Not in scope**: operation semantics, query correctness, perf fast
  paths, version validation. Those belong in Rust.
- **Async testing**: Uses pytest-asyncio for async/await support.

## Test Execution

### Rust Tests

Run from project root:
```bash
cargo test  # Run all Rust tests
```

**Working directory:** Tests run from project root, so paths like `"test_data/userdata.parquet"` work directly

### Python Tests

Run from project root:
```bash
poetry install  # Install dependencies
poetry run pytest  # Run Python E2E tests
```

**Setup:** Uses `maturin_import_hook` to auto-compile Rust code on import

**Working directory:** pytest runs from project root, same path convention as Rust tests

## Ignored Tests Document Future Behavior

Tests marked `#[ignore]` specify expected behavior that should be implemented:

```rust
#[tokio::test]
#[ignore]  // TODO: Implement validation
async fn test_remove_nonexistent_column_error() -> Result<(), ContainerError> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_str().unwrap();

    let mut container = Bundlebase::create(temp_path).await?;
    container.attach("test_data/userdata.parquet").await?;

    // Should fail when removing a column that doesn't exist
    let result = container.drop_column("nonexistent_column").await;
    assert!(result.is_err());  // Expected behavior

    Ok(())
}
```

These serve as specification and regression tests for future features.

## Development Workflow

**Testing best practices:**
1. Always start with Rust code and ensure it's working and well tested — most coverage lives in `rust/bundlebase/tests/`
2. THEN, if you touched the Python binding surface, add a thin Python test for the binding itself
3. Python tests are high-level E2E checks of the Python binding only
4. Python tests MUST NOT re-test underlying Rust business logic — add a Rust test instead

**Test data:**
- All test data in `test_data/` directory at project root
- Shared between Rust and Python tests
- Currently includes: `userdata.parquet` and other test fixtures

## Running Tests with Output

### Rust Tests with Output

```bash
# Show println! output
cargo test -- --nocapture

# Run specific test
cargo test test_name -- --nocapture

# Run with backtrace
RUST_BACKTRACE=1 cargo test
```

### Python Tests with Output

```bash
# Show pytest output
poetry run pytest -v

# Run specific test
poetry run pytest -v tests/test_e2e.py::test_name

# Show print statements
poetry run pytest -s
```
