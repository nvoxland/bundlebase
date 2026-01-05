# Template: Fix Bug

Use this template when investigating and fixing a bug in bundlebase. This template guides you through systematic debugging and ensures the fix doesn't introduce new issues.

## When to Use This Template

- Data corruption or incorrect results
- Crashes, panics, or exceptions
- Memory leaks or performance degradation
- Type errors or API mismatches
- Unexpected behavior in edge cases

## Required Reading

Before starting, read:

1. **[anti-patterns.md](../anti-patterns.md)** - Common mistakes that cause bugs
2. **[ai-rules.md](../ai-rules.md)** - Constraints that prevent bugs
3. **[testing.md](../testing.md)** - How to write regression tests

Context-specific reading (based on bug area):
- Operations bugs: [decisions/006-lazy-evaluation.md](../decisions/006-lazy-evaluation.md)
- Memory issues: [decisions/003-streaming-only.md](../decisions/003-streaming-only.md)
- Python errors: [python-bindings.md](../python-bindings.md)

## Critical Constraints

When fixing bugs:

- ✅ **Write regression test first** - Reproduce bug, then fix
- ✅ **Don't break existing tests** - All tests must still pass
- ✅ **Follow constraints** - No `.unwrap()`, no `collect()`, etc.
- ✅ **Add context to errors** - Improve error messages
- ✅ **Document root cause** - Comment explaining the fix

## Investigation Checklist

### 1. Reproduce the Bug

- [ ] Get minimal reproduction case
- [ ] Identify affected versions
- [ ] Determine if it's Rust-side or Python-side
- [ ] Check if it's a regression (did it ever work?)
- [ ] Document exact error message or incorrect behavior

**Example:**
```python
# Minimal reproduction
c = bundlebase.create()
c.attach("data.parquet")
c.filter("age > 18")  # ❌ Crashes here
```

### 2. Write Failing Test

- [ ] Create test that reproduces the bug
- [ ] Test should FAIL before fix, PASS after fix
- [ ] Use appropriate test type:
  - Rust unit test for Rust bugs (`#[test]`)
  - Python E2E test for Python bugs (`python/tests/`)
- [ ] Keep test minimal and focused

**Example:**
```python
# python/tests/test_bug_filter_crash.py

def test_filter_with_special_characters():
    """Regression test for issue #123 - filter crashes on quotes."""
    c = bundlebase.create()
    c.attach("tests/data/sample.parquet")

    # This should not crash
    c.filter("name = 'O\\'Brien'")

    df = c.to_pandas()
    assert len(df) >= 0  # Should complete without error
```

### 3. Investigate Root Cause

- [ ] Add logging to trace execution (see [logging.md](../logging.md))
- [ ] Check for common anti-patterns (see [anti-patterns.md](../anti-patterns.md))
- [ ] Review related code for similar issues
- [ ] Check if error handling is missing
- [ ] Verify streaming execution is used (no `collect()`)
- [ ] Look for `.unwrap()` or `.expect()` calls

**Investigation techniques:**
```rust
// Add debug logging
log::debug!("Processing filter expression: {}", expr);

// Check intermediate values
let parsed = parse_sql(expr)?;
log::debug!("Parsed SQL: {:?}", parsed);

// Verify streaming
let stream = df.execute_stream().await?;  // ✅ Should see this
let data = df.collect().await?;  // ❌ If you see this, that's the bug
```

### 4. Identify Fix Strategy

Choose the appropriate fix:

- [ ] **Missing validation** - Add check in `check()` phase
- [ ] **Error handling** - Replace `.unwrap()` with `?` or `.ok_or()`
- [ ] **Streaming violation** - Replace `collect()` with `execute_stream()`
- [ ] **Schema mismatch** - Fix `reconfigure()` to update schema correctly
- [ ] **Python binding** - Fix Arc cloning or error mapping
- [ ] **Logic error** - Fix algorithm or condition

### 5. Implement Fix

- [ ] Make minimal changes (don't refactor unrelated code)
- [ ] Follow all critical constraints (no `.unwrap()`, etc.)
- [ ] Add error context to error messages
- [ ] Add comment explaining WHY the fix works
- [ ] Update related code if pattern is repeated

**Example fix:**
```rust
// Before (buggy)
pub fn filter(&mut self, expr: &str) -> Result<&mut Self> {
    let parsed = parse_sql(expr).unwrap();  // ❌ Panics on bad SQL
    // ...
}

// After (fixed)
pub fn filter(&mut self, expr: &str) -> Result<&mut Self> {
    // Parse SQL expression, returning error if invalid
    let parsed = parse_sql(expr)
        .map_err(|e| format!("Invalid filter expression '{}': {}", expr, e))?;  // ✅ Proper error handling
    // ...
}
```

### 6. Verify Fix

- [ ] Run the failing test - should now PASS
- [ ] Run all tests - should still pass
- [ ] Test edge cases related to the bug
- [ ] Verify no performance regression
- [ ] Check memory usage (for memory-related bugs)

**Commands:**
```bash
# Run specific test
poetry run pytest tests/test_bug_filter_crash.py -v

# Run all Python tests
poetry run pytest

# Run Rust tests
cargo test

# Check for clippy warnings
cargo clippy
```

### 7. Documentation

- [ ] Add comment explaining the fix in the code
- [ ] Update error messages to be more helpful
- [ ] If bug reveals missing documentation, update relevant `.context/` file
- [ ] Consider if bug suggests new anti-pattern for [anti-patterns.md](../anti-patterns.md)

**Example comment:**
```rust
// Fix for issue #123: SQL expressions with single quotes need escaping
// Before this fix, quotes would cause parser to crash
let escaped = expr.replace("'", "\\'");
let parsed = parse_sql(&escaped)?;
```

## Common Bug Patterns

### 1. Unwrap Panic

**Symptom:** "thread 'main' panicked at 'called `Option::unwrap()` on a `None` value'"

**Root cause:** Using `.unwrap()` instead of proper error handling

**Fix:**
```rust
// Before
let value = option.unwrap();  // ❌

// After
let value = option.ok_or_else(|| "Value not found".into())?;  // ✅
```

### 2. Memory Exhaustion

**Symptom:** Program runs out of memory on large datasets

**Root cause:** Using `collect()` instead of streaming

**Fix:**
```rust
// Before
let batches = df.collect().await?;  // ❌ Loads entire dataset

// After
let stream = df.execute_stream().await?;  // ✅ Streaming
while let Some(batch) = stream.next().await {
    // Process incrementally
}
```

### 3. Schema Mismatch

**Symptom:** "Column 'x' not found" after operation that should preserve it

**Root cause:** `reconfigure()` not updating schema correctly

**Fix:**
```rust
// Before
fn reconfigure(&self, state: &mut BundleState) -> Result<()> {
    // ❌ Schema not updated after removing column
    Ok(())
}

// After
fn reconfigure(&self, state: &mut BundleState) -> Result<()> {
    // ✅ Update schema to reflect removed column
    let new_schema = state.schema().project_exclude(&[&self.column])?;
    state.set_schema(new_schema);
    Ok(())
}
```

### 4. Python Exception Without Context

**Symptom:** Generic "RuntimeError" in Python with no details

**Root cause:** Not mapping Rust error with context

**Fix:**
```rust
// Before
self.inner.filter(&expr)?;  // ❌ Generic error

// After
self.inner.filter(&expr)
    .map_err(|e| PyErr::new::<PyRuntimeError, _>(
        format!("Filter '{}' failed: {}", expr, e)  // ✅ Context added
    ))?;
```

### 5. Arc Clone Missing

**Symptom:** Lifetime errors or "value moved" in Python bindings

**Root cause:** Not cloning Arc wrapper for Python

**Fix:**
```rust
// Before
#[pymethods]
impl PyBundleBuilder {
    fn filter(&mut self, expr: String) -> PyResult<&mut Self> {
        self.inner.filter(&expr)?;
        Ok(self)  // ❌ Can't return Rust reference to Python
    }
}

// After
#[pymethods]
impl PyBundleBuilder {
    fn filter(&mut self, expr: String) -> PyResult<Self> {
        self.inner.filter(&expr)?;
        Ok(self.clone())  // ✅ Clone Arc (cheap)
    }
}
```

## Testing Checklist

After fix, verify:

- [ ] Regression test passes (bug is fixed)
- [ ] All existing Rust tests pass (`cargo test`)
- [ ] All existing Python tests pass (`poetry run pytest`)
- [ ] No new clippy warnings (`cargo clippy`)
- [ ] Related edge cases tested
- [ ] Performance not degraded
- [ ] Memory usage not increased

## Code Review Checklist

- [ ] Regression test added and passes
- [ ] All tests still pass
- [ ] Fix is minimal (doesn't refactor unrelated code)
- [ ] No new `.unwrap()` or `collect()` calls
- [ ] Error messages include context
- [ ] Comment explains WHY the fix works
- [ ] Related code checked for same bug
- [ ] Documentation updated if needed

## Example: Fixing Filter Crash on Special Characters

### 1. Bug Report
```
User reports: filter("name = 'O'Brien'") crashes with parse error
```

### 2. Reproduce
```python
# Minimal reproduction
c = bundlebase.create()
c.attach("data.parquet")
c.filter("name = 'O'Brien'")  # ❌ Crashes
```

### 3. Write Failing Test
```python
def test_filter_with_apostrophe():
    c = bundlebase.create()
    c.attach("tests/data/names.parquet")
    c.filter("name = 'O\\'Brien'")  # Should not crash
    df = c.to_pandas()
    assert len(df) >= 0
```

### 4. Investigate
```rust
// Found the issue in filter operation
fn check(&self, state: &BundleState) -> Result<()> {
    parse_sql(&self.expression)?;  // ❌ Doesn't handle escaped quotes
    Ok(())
}
```

### 5. Fix
```rust
fn check(&self, state: &BundleState) -> Result<()> {
    // Parse SQL, allowing escaped quotes in string literals
    parse_sql_with_escapes(&self.expression)
        .map_err(|e| format!("Invalid SQL expression '{}': {}", self.expression, e))?;
    Ok(())
}
```

### 6. Verify
```bash
poetry run pytest tests/test_filter.py::test_filter_with_apostrophe  # ✅ Passes
poetry run pytest  # ✅ All tests pass
```

## Success Criteria

Bug is fixed when:

- ✅ Regression test added and passes
- ✅ Root cause identified and documented
- ✅ Fix implemented with minimal changes
- ✅ All existing tests still pass
- ✅ No new violations of critical constraints
- ✅ Error messages improved
- ✅ Code review checklist complete

## Related Templates

- [performance-review.md](performance-review.md) - If bug is performance-related
- [add-operation.md](add-operation.md) - If bug reveals missing operation validation

## Related Documentation

- [anti-patterns.md](../anti-patterns.md) - Common bugs to avoid
- [testing.md](../testing.md) - Testing strategy
- [decisions/007-no-unwrap.md](../decisions/007-no-unwrap.md) - Why no `.unwrap()`
