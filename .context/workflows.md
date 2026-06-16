# Development Workflows

Step-by-step guides for common development tasks in the Bundlebase project. Follow these procedures to ensure consistency and completeness.

## 1. Adding a New Feature

### Step 1: Implement in Rust

**Location**: `rust/bundlebase/src/`

**Process**:
1. Identify the appropriate module or create a new one
2. Implement the feature in pure Rust (no PyO3 code)
3. Ensure proper error handling using `?` operator (no `.unwrap()`)
4. Modification methods take `&self` and return `Result<&Self>` (interior mutability, enables chaining)
5. Use `execute_stream()` for any DataFrame operations (never `collect()`)

**Example** (adding a new transformation — see [prompts/add-operation.md](prompts/add-operation.md) for the full operation workflow):
```rust
// 1. New op in rust/bundlebase/src/bundle/operation/deduplicate.rs
//    implementing the Operation trait (check / apply / apply_dataframe),
//    registered via define_any_operation! in operation.rs.

// 2. Convenience method on the BundleBuilderExt trait
//    (rust/bundlebase-command/src/builder_ext.rs):
async fn deduplicate(&self, columns: Vec<String>) -> Result<&Self, BundlebaseError> {
    // Runs the command through run_command() for change tracking;
    // the command's execute() validates input and calls
    // builder.apply_operation(DeduplicateOp::setup(...).into())
    exec_cmd(self, DeduplicateCommand::new(columns)).await?;
    Ok(self)
}
```

**Checklist**:
- [ ] No `.unwrap()` calls (enforced by compiler)
- [ ] Proper error handling with `?`
- [ ] Modification methods are `&self -> Result<&Self>` and routed through change tracking
- [ ] No PyO3 imports in core Rust code
- [ ] Streaming execution if DataFrame operations involved

### Step 2: Add Rust Tests

**Location**:
- Unit tests: `rust/bundlebase/src/` (in same file or `tests/` submodule)
- Integration tests: `rust/bundlebase/tests/`

**Process**:
1. Write unit tests for individual functions
2. Write integration tests for full workflows
3. Test error cases and edge conditions
4. Verify streaming behavior if applicable

**Example**:
```rust
// rust/bundlebase/tests/deduplicate.rs
#[tokio::test]
async fn test_deduplicate_removes_duplicates() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let path = temp_dir.path().to_str().unwrap();

    let builder = BundleBuilder::create(path, None).await?;
    builder.attach("test_data/with_dupes.parquet", None).await?;

    // Apply deduplication
    builder.deduplicate(vec!["id".to_string()]).await?;

    // Verify results
    assert_eq!(builder.num_rows().await?, expected_count);

    Ok(())
}
```

**Run tests**:
```bash
cargo test deduplicate
```

**Checklist**:
- [ ] Unit tests for new functions
- [ ] Integration test for full workflow
- [ ] Error case testing
- [ ] All tests passing

### Step 3: Add Python Bindings

**Location**: `rust/bundlebase-python/src/`

**Process**:
1. Add `#[pymethods]` wrapper in the appropriate PyClass
2. Clone the inner `Arc`-shared builder, then await the Rust call inside `pyo3_async_runtimes::tokio::future_into_py`
3. Convert errors to Python exceptions
4. Make method async for consistency

**Example** (pattern from `rust/bundlebase-python/src/builder.rs`):
```rust
#[pymethods]
impl PyBundleBuilder {
    fn deduplicate<'py>(
        slf: PyRef<'_, Self>,
        columns: Vec<String>,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = slf.inner.clone(); // Arc-shared builder
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner.deduplicate(columns).await.map_err(to_py_err)?;
            Ok(())
        })
    }
}
```

**Checklist**:
- [ ] PyO3 code only in `bundlebase-python/` crate
- [ ] Proper error conversion to Python exceptions
- [ ] Inner builder cloned (Arc) before the async block — no references across await
- [ ] Consistent async/await pattern

### Step 4: Register in Python Layer

**Location**: `python/src/bundlebase/__init__.py`

**Process**:
1. Add the PyO3 method to the `_original_methods` dict
2. If it mutates the bundle and returns the builder, add it to the `mutation_methods` list so it's wrapped to return an `OperationChain`
3. Ensure method name matches Rust `#[pyo3(name = "...")]` attribute

**Example**:
```python
# python/src/bundlebase/__init__.py

_original_methods = {
    ...
    "deduplicate": _PyBundleBuilder.deduplicate,
}

mutation_methods = [
    "attach", "drop_column", ...,
    "deduplicate",
]
```

**Checklist**:
- [ ] Method added to `_original_methods`
- [ ] Mutation methods added to `mutation_methods` (enables chaining + sync API)
- [ ] Read-only methods that exist on both PyBundle and PyBundleBuilder are handled separately (see comments in `__init__.py`)

### Step 5: Write Python E2E Tests

**Location**: `python/tests/test_e2e.py`

**Process**:
1. Add test function with `async` keyword
2. Test that Python binding works end-to-end
3. Verify both async and sync APIs work
4. DO NOT test Rust business logic (that's in Rust tests)

**Example**:
```python
# python/tests/test_e2e.py
import pytest
import bundlebase
import bundlebase.sync as dc

@pytest.mark.asyncio
async def test_deduplicate_async():
    """Test that deduplicate binding works (async)"""
    c = await bundlebase.create()
    await c.attach("test_data/with_dupes.parquet")

    # Call deduplicate
    c = c.deduplicate(["id"])

    # Verify it returns results
    df = await c.to_pandas()
    assert len(df) > 0  # Basic verification

def test_deduplicate_sync():
    """Test that deduplicate binding works (sync)"""
    c = dc.create()
    c.attach("test_data/with_dupes.parquet")

    # Call deduplicate (no await)
    c = c.deduplicate(["id"])

    # Verify it returns results
    df = c.to_pandas()
    assert len(df) > 0
```

**Run tests**:
```bash
poetry run pytest -v -k deduplicate
```

**Checklist**:
- [ ] Async test added
- [ ] Sync test added
- [ ] Tests verify binding works, not Rust logic
- [ ] All Python tests passing

### Step 6: Update Documentation

**Files to update**:
- `/.context/python-api.md` - Add API documentation
- `/.context/glossary.md` - Add any new terminology
- `/README.md` - Add example if it's a major feature
- `/docs/` - Update user-facing documentation

**Example**:
```markdown
# .context/python-api.md

## Deduplication

Remove duplicate rows based on column values.

### deduplicate(columns)

**Parameters**:
- `columns` (list[str]): Column names to use for deduplication

**Returns**: Self (for chaining)

**Example**:
\`\`\`python
c = c.deduplicate(["id", "email"])
\`\`\`
```

**Checklist**:
- [ ] API documentation added
- [ ] Glossary updated if new terms introduced
- [ ] Examples added for significant features
- [ ] Cross-references to related features

### Step 7: Verify Everything

**Final verification**:
```bash
# Build and test Rust
cargo test

# Build Python extension
./scripts/maturin-dev.sh

# Test Python bindings
poetry run pytest

# Run specific feature tests
cargo test deduplicate
poetry run pytest -k deduplicate
```

**Checklist**:
- [ ] All Rust tests passing
- [ ] All Python tests passing
- [ ] Feature works in both async and sync APIs
- [ ] Documentation updated
- [ ] No clippy warnings: `cargo clippy`

## 2. Running Tests

### Local Test Execution

**All tests**:
```bash
# Rust tests (use the test profile — see CLAUDE.md build guidance)
cargo test --profile test

# Python tests
poetry run pytest python/tests/

# Both
cargo test --profile test && poetry run pytest python/tests/
```

**Specific test**:
```bash
# Rust - specific test by name
cargo test test_filter_basic

# Rust - specific module
cargo test filters_selects

# Python - specific test
poetry run pytest -k test_filter_async

# Python - specific file
poetry run pytest python/tests/test_e2e.py
```

### Test Debugging

**Rust with output**:
```bash
# Show println! output
cargo test -- --nocapture

# With backtrace
RUST_BACKTRACE=1 cargo test

# Full backtrace
RUST_BACKTRACE=full cargo test

# Specific test with output
cargo test test_name -- --nocapture
```

**Python with output**:
```bash
# Verbose output
poetry run pytest -v

# Show print statements
poetry run pytest -v -s

# Stop on first failure
poetry run pytest -x

# Show locals on failure
poetry run pytest -l
```

**Test with timeout**:
```bash
# Python tests with timeout (useful for hanging tests)
poetry run pytest --timeout=30
```

### Continuous Testing During Development

**Rust watch mode**:
```bash
# Install cargo-watch
cargo install cargo-watch

# Auto-run tests on file changes
cargo watch -x test
```

**Python watch mode**:
```bash
# Install pytest-watch
poetry add --dev pytest-watch

# Auto-run tests on file changes
poetry run ptw
```

## 3. Building and Releasing

### Development Build

**Quick rebuild** (during development):
```bash
# Build Rust extension and install in dev mode
./scripts/maturin-dev.sh

# Or via Poetry
poetry run ./scripts/maturin-dev.sh
```

**Clean build** (after major changes):
```bash
# Clean previous builds
cargo clean

# Rebuild everything
./scripts/maturin-dev.sh
```

### Release Build

**Build Python wheel**:
```bash
# Build release wheel
./scripts/maturin-build.sh --release

# Output: target/wheels/bundlebase-*.whl
```

**Build for multiple Python versions**:
```bash
# Build for all installed Python versions
./scripts/maturin-build.sh --release --interpreter python3.9 python3.10 python3.11
```

**Build source distribution**:
```bash
# Build source tarball
maturin sdist
```

### Testing Release Build

**Install wheel locally**:
```bash
# Build wheel
./scripts/maturin-build.sh --release

# Install in fresh environment
python -m venv test_env
source test_env/bin/activate
pip install target/wheels/bundlebase-*.whl

# Test import
python -c "import bundlebase; print(bundlebase.__version__)"
```

## 4. Adding Dependencies

### Rust Dependencies

**Add to workspace**:
```bash
# Edit Cargo.toml [workspace.dependencies] section
# Example: serde = { version = "1.0", features = ["derive"] }
```

**Use in crate**:
```toml
# In rust/bundlebase/Cargo.toml
[dependencies]
serde = { workspace = true }
```

**Verify**:
```bash
cargo check
cargo test
```

### Python Dependencies

**Runtime dependency**:
```bash
poetry add package_name
```

**Development dependency**:
```bash
poetry add --group dev package_name
```

**Optional dependency**:
```bash
poetry add --optional package_name
```

**Update lockfile**:
```bash
poetry update
```

## 5. Debugging Performance Issues

### Check for collect() Usage

**Search for anti-pattern**:
```bash
# Find all collect() calls in Rust code
rg "\.collect\(\)" --type rust rust/bundlebase/src/

# Should return very few or zero results
```

**Replace with streaming**:
```rust
// Before (BAD)
let batches = df.collect().await?;

// After (GOOD)
let stream = df.execute_stream().await?;
```

### Memory Profiling

**Rust memory profiling**:
```bash
# Install heaptrack
# Ubuntu: sudo apt install heaptrack
# macOS: brew install heaptrack

# Profile Rust tests
heaptrack cargo test

# Analyze results
heaptrack --analyze heaptrack.cargo.*.gz
```

**Python memory profiling**:
```python
# Install memory-profiler
poetry add --dev memory-profiler

# Add decorator to function
from memory_profiler import profile

@profile
async def test_memory_usage():
    c = await bundlebase.open("large_file.parquet")
    df = await c.to_pandas()

# Run with profiler
poetry run python -m memory_profiler test_script.py
```

### Performance Testing

**Time operations**:
```rust
// Rust - use Instant
use std::time::Instant;

let start = Instant::now();
let result = expensive_operation().await?;
let duration = start.elapsed();
println!("Operation took: {:?}", duration);
```

```python
# Python - use time module
import time

start = time.time()
result = await expensive_operation()
duration = time.time() - start
print(f"Operation took: {duration:.2f}s")
```

## 6. Git Workflow

### Checking Status

**Before any changes**:
```bash
# See current branch and changes
git status

# See what will be committed
git diff

# See staged changes
git diff --staged
```

### Committing Changes

**IMPORTANT**: Only commit when explicitly requested by the user!

**Standard commit**:
```bash
# Stage files
git add .

# Commit with message
git commit -m "Add deduplicate operation

- Implement deduplicate in Rust
- Add Python bindings
- Add tests and documentation"

# Verify commit
git log -1
```

**Commit message format**:
- First line: Brief summary (50 chars or less)
- Blank line
- Detailed description with bullet points
- Reference issues if applicable

### Creating Pull Requests

**Before PR**:
```bash
# Ensure all tests pass
cargo test && poetry run pytest

# Check for warnings
cargo clippy

# Format code
cargo fmt
```

**PR description template**:
```markdown
## Summary
Brief description of changes

## Changes
- Bullet point list of changes

## Testing
- [ ] Rust tests added/updated
- [ ] Python tests added/updated
- [ ] Documentation updated
- [ ] All tests passing

## Related Issues
Fixes #123
```

## 7. Documentation Updates

### When to Update .context Files

**Trigger events**:
- Adding new architectural pattern → Update `architecture.md`
- Adding new Python API → Update `python-api.md`
- Changing PyO3 patterns → Update `python-bindings.md`
- New workflow established → Update this file
- New constraints added → Update `ai-rules.md`
- New anti-pattern discovered → Update `anti-patterns.md`
- New terminology introduced → Update `glossary.md`

### Keeping Examples Current

**Verify examples compile/run**:
```bash
# Extract code examples from markdown
# (Manual process - copy/paste to test file)

# Test Rust examples
cargo test --doc

# Test Python examples
poetry run python docs/examples/example.py
```

**Review checklist**:
- [ ] All code examples are runnable
- [ ] API signatures match current implementation
- [ ] No deprecated methods in examples
- [ ] Examples demonstrate best practices

### Documentation Structure

**Each .context file should have**:
1. Purpose & overview
2. Core concepts explained
3. Code examples (working & tested)
4. References to related docs
5. Decision rationale where applicable

## 8. Troubleshooting

### Build Failures

**Rust compile error**:
```bash
# Clean and rebuild
cargo clean
cargo test --no-run

# Check specific error
cargo check

# Fix formatting issues
cargo fmt
```

**Python import error**:
```bash
# Rebuild extension
./scripts/maturin-dev.sh

# Verify installation
python -c "import bundlebase; print(bundlebase._bundlebase)"
```

### Test Failures

**Rust test hanging**:
```bash
# Run with timeout
cargo test -- --test-threads=1 --nocapture

# Check for deadlocks (if using locks)
RUST_LOG=debug cargo test
```

**Python test failing**:
```bash
# Run with verbose output
poetry run pytest -vv -s

# Run single test
poetry run pytest -k test_name -vv

# Check for async issues
# Ensure @pytest.mark.asyncio decorator present
```

### Environment Issues

**Reset environment**:
```bash
# Remove build artifacts
cargo clean
rm -rf target/

# Reinstall Python dependencies
poetry env remove python
poetry install

# Rebuild extension
./scripts/maturin-dev.sh
```

---

**See Also**:
- [development.md](development.md) - Development setup and philosophy
- [ai-rules.md](ai-rules.md) - Hard constraints to follow
- [anti-patterns.md](anti-patterns.md) - What NOT to do
- [testing.md](testing.md) - Testing strategy details
