# Testing Strategy

## Test Coverage Overview

**Rust Lib Tests** (`src/`):
- **Total tests**: ~150+ (all passing)
- **Coverage**: Module unit tests, data storage tests, function registry tests, schema tracking, versioning, row indexing

**Rust Integration Tests** (`tests/`):
- **Test files**: `basic.rs`, `operations.rs`, `functions.rs`, `filters_selects.rs`, `joins.rs`, `queries.rs`, `schema.rs`, `extending.rs`
- **Passing tests**: Cover all operations (attach, remove, rename, filter, select, join, commit/open, functions, metadata, row indexing)
- **Test organization**: Split by feature area for focused testing
- **Common utilities**: `tests/common/mod.rs` provides shared test helpers

**Python E2E Tests** (`python/tests/test_e2e.py`):
- **Coverage**: Binding verification, file formats (Parquet/CSV/JSON), conversions (pandas, polars, dict, numpy), custom functions, metadata, schema introspection, commit/open roundtrip
- **Test strategy**: Python tests verify binding layer works; Rust tests verify operation logic
- **Async testing**: All Python tests use pytest-asyncio for async/await support

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
    let result = container.remove_column("nonexistent_column").await;
    assert!(result.is_err());  // Expected behavior

    Ok(())
}
```

These serve as specification and regression tests for future features.

## Development Workflow

**Testing best practices:**
1. Always start with Rust code and ensure it's working and well tested
2. THEN write Python code and tests
3. Python tests should be E2E tests focusing on the Python binding
4. Python tests should NOT test underlying Rust business logic (that's covered by Rust tests)

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
