# Development Guide

## Setup

### Install Poetry

If not already installed:
```bash
curl -sSL https://install.python-poetry.org | python3 -
```

### Install Dependencies

```bash
poetry install  # Install Python dependencies
```

## Build and Development

### Build for Development

**CRITICAL: Always use the maturin wrapper scripts**

Compile Rust extension and install locally:

```bash
# CORRECT: Use the wrapper script (prevents full rebuilds)
./scripts/maturin-dev.sh

# WRONG: Never use maturin directly - causes full rebuilds when switching with cargo
# maturin develop  ❌ DO NOT USE
```

**Why the script?** Maturin and cargo use different feature flags (`extension-module`), causing full rebuilds of 200+ crates when switching between them. The wrapper script uses `target/maturin/` instead of `target/debug/` to keep build artifacts separate.

**For Rust-only development:**
```bash
# Fast, doesn't trigger Python rebuilds
cargo build --package bundlebase
cargo test --package bundlebase
```

See [scripts/README.md](../scripts/README.md) for technical details.

### Build Release Version

```bash
./scripts/maturin-build.sh --release
```

## Development Workflow

### Step 1: Implement Rust Code

- Write the Rust implementation
- Add comprehensive Rust tests
- Ensure all Rust tests pass
- Run Rust tests: `cargo test`

### Step 2: Write Python Bindings

- Add PyO3 bindings in `src/python/`
- Remember: PyO3 code should only be used in the python module
- Don't create mod.rs files
- Use proper error handling with `.map_err()` to convert Rust errors to Python exceptions

### Step 3: Write Python Tests

- Write E2E tests in `python/tests/test_e2e.py`
- Python tests should verify the Python binding works
- Python tests should NOT test underlying Rust business logic
- Use pytest-asyncio for async/await support
- All Python conversion methods are async and must be awaited

### Step 4: Verify All Tests Pass

```bash
# Test Rust code
cargo test --package bundlebase

# Build Python package and run tests
./scripts/maturin-dev.sh
poetry run pytest python/tests/
```

## Dependency Management

### Add Python Dependency

```bash
poetry add <package>
```

### Add Development Dependency

```bash
poetry add --group dev <package>
```

### Update Dependencies

```bash
poetry update
```

## Project Philosophy

- **No compatibility mode**: The project is not launched, so never need to keep methods purely for compatibility reasons
- **Simplicity over flexibility**: Only add features that are directly requested or clearly necessary
- **No over-engineering**: Don't add error handling, fallbacks, or validation for scenarios that can't happen
- **Minimal abstractions**: Don't create helpers, utilities, or abstractions for one-time operations
- **Trust internals**: Trust internal code and framework guarantees; only validate at system boundaries (user input, external APIs)
- **Streaming first**: Always use streaming execution for query results - never materialize datasets in memory

## Performance Best Practices

### Critical: Always Use Streaming Execution

**Rust code:**
```rust
// ✅ GOOD: Streaming execution (constant memory)
let stream = dataframe.execute_stream().await?;
let py_stream = PyRecordBatchStream::new(stream, schema);

// ❌ BAD: Materializes entire dataset (3x memory usage)
let batches = dataframe.collect().await?;  // NEVER DO THIS
```

**Python bindings:**
```rust
// ✅ GOOD: Return streaming object
fn as_pyarrow_stream<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
    let stream = dataframe.execute_stream().await?;  // Stream
    // Return PyRecordBatchStream
}

// ❌ BAD: Materialize before returning
fn as_pyarrow<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
    let batches = dataframe.collect().await?;  // Avoid for large data
    // ...
}
```

**Why this matters:**
- **Old behavior** (`collect()`): 10GB file = ~30GB peak RAM (3x dataset size)
- **New behavior** (streaming): 10GB file = ~50MB peak RAM (constant batch size)

### Python API Development

When adding new conversion methods:

1. **Use streaming internally:**
   ```python
   async def to_new_format(container):
       # Stream batches, process incrementally
       results = []
       async for batch in stream_batches(container):
           chunk = process_batch(batch)  # Process one at a time
           results.append(chunk)
       return combine(results)
   ```

2. **Don't materialize first:**
   ```python
   # ❌ BAD: Defeats streaming
   async def to_new_format(container):
       arrow_table = await _get_arrow_table(container)  # Full materialization
       return convert(arrow_table)
   ```

### Code Review Checklist

Before committing Rust code:
- [ ] Uses `execute_stream()` instead of `collect()`
- [ ] Returns `PyRecordBatchStream` to Python, not `Vec<RecordBatch>`
- [ ] No intermediate `Vec<RecordBatch>` accumulation
- [ ] Tests verify streaming behavior (check memory, not just correctness)

Before committing Python code:
- [ ] Uses `stream_batches()` or automatic streaming (`to_pandas`/`to_polars`)
- [ ] Doesn't accumulate all batches in a list
- [ ] Processes batches incrementally when possible
- [ ] Documents memory characteristics in docstrings

## File Organization

- **Rust source**: `src/` directory
- **Python bindings**: `src/python/` directory
- **Python package**: `python/Bundlebase/` directory
- **Rust tests**: `tests/` directory
- **Python tests**: `python/tests/test_e2e.py`
- **Test data**: `test_data/` directory
- **Configuration**: `Cargo.toml`, `pyproject.toml`

## Common Commands

```bash
# Build development version
maturin develop

# Run all tests
cargo test && poetry run pytest

# Run Rust tests only
cargo test

# Run Python tests only
poetry run pytest

# Run Python tests with output
poetry run pytest -v -s

# Build release
maturin build --release
```

## Git Workflow

- **Do NOT commit** unless explicitly told to do so
- Check git status: `git status`
- Preview changes: `git diff`

## Container Types in Code

When implementing features:

**For read-only operations:**
- Use `&self` methods on any container type

**For mutation operations:**
- Only work with BundlebaseBuilder
- Use `&mut self` methods that return `&mut Self`
- Operations modify container in place and can be chained

**For serialization:**
- Use `Operation::config()` to serialize operations
- Deserialize via `OperationRegistry` to reconstruct operations
