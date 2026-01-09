# Development Workflows

Step-by-step guides for common development tasks in the Bundlebase project. Follow these procedures to ensure consistency and completeness.

## 1. Adding a New Feature

### Step 1: Implement in Rust

**Location**: `rust/bundlebase/src/`

**Process**:
1. Identify the appropriate module or create a new one
2. Implement the feature in pure Rust (no PyO3 code)
3. Ensure proper error handling using `?` operator (no `.unwrap()`)
4. Return `&mut Self` for mutable operations (enables chaining)
5. Use `execute_stream()` for any DataFrame operations (never `collect()`)

**Example** (adding a new transformation):
```rust
// rust/bundlebase/src/bundle.rs
impl BundleBuilder {
    pub fn deduplicate(&mut self, columns: Vec<String>) -> Result<&mut Self> {
        // Validate columns exist
        for col in &columns {
            if !self.schema().column_with_name(col).is_some() {
                return Err(format!("Column not found: {}", col).into());
            }
        }

        // Record operation (lazy evaluation)
        self.operations.push(Operation::Deduplicate(columns));

        // Update state if needed
        // Schema unchanged, row count may decrease

        Ok(self)
    }
}
```

**Checklist**:
- [ ] No `.unwrap()` calls (enforced by compiler)
- [ ] Proper error handling with `?`
- [ ] Mutable operations return `&mut Self`
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

    let mut container = BundlebaseBuilder::create(path).await?;
    container.attach("test_data/with_dupes.parquet").await?;

    // Apply deduplication
    container.deduplicate(vec!["id".to_string()]).await?;

    // Verify results
    let stream = container.execute_stream().await?;
    // Check row count decreased...

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
1. Add `#[pymethod]` wrapper in appropriate PyClass
2. Convert Rust types to Python types
3. Handle errors with `.map_err()` to convert to PyErr
4. Make method async for consistency
5. Return `Self` (not `&mut Self`) to satisfy Python's ownership model

**Example**:
```rust
// rust/bundlebase-python/src/bundle.rs
#[pymethods]
impl PyBundleBuilder {
    fn deduplicate(&mut self, columns: Vec<String>) -> PyResult<Self> {
        self.inner
            .deduplicate(columns)
            .map_err(|e| PyErr::new::<PyRuntimeError, _>(format!("Deduplicate error: {}", e)))?;

        // Return cloned self for Python chaining
        Ok(self.clone())
    }
}
```

**Checklist**:
- [ ] PyO3 code only in `bundlebase-python/` crate
- [ ] Proper error conversion with `.map_err()`
- [ ] Method returns Self for Python chaining
- [ ] Consistent async/await pattern

### Step 4: Register in Python Layer

**Location**: `python/src/bundlebase/__init__.py`

**Process**:
1. Register async method on both `Bundle` and `BundleBuilder` classes
2. Register sync wrapper for `bundlebase.sync` module
3. Ensure method name matches Rust `#[pyo3(name = "...")]` attribute

**Example**:
```python
# python/src/bundlebase/__init__.py

# Register on both classes
for cls in [Bundle, BundleBuilder]:
    # Async method
    register_async_method(cls, 'deduplicate', 'deduplicate')

    # Sync wrapper
    register_sync_method(cls, 'deduplicate', 'deduplicate')
```

**Checklist**:
- [ ] Method registered for async API
- [ ] Method registered for sync API
- [ ] Registration on both Bundle and BundleBuilder if applicable

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
# Rust tests
cargo test

# Python tests
poetry run pytest

# Both
cargo test && poetry run pytest
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
cargo build

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
