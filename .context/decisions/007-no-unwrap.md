# ADR-007: No .unwrap() Allowed

**Status:** Accepted

**Date:** December 2024 (documented retroactively)

## Context

Rust's `Option` and `Result` types require explicit handling. The `.unwrap()` method provides a shortcut but panics if the value is `None` or `Err`, potentially crashing the program.

### The Problem

`.unwrap()` is dangerous in library code:
- **Panics are unrecoverable** in normal use (can't be caught by caller)
- **Poor error messages** ("thread 'main' panicked at 'called `Option::unwrap()` on a `None` value'")
- **Production crashes** when encountering unexpected inputs
- **No error context** about why the unwrap failed

### Alternatives Considered

**Option 1: Allow .unwrap() everywhere**
- Pros: Convenient during development, less code
- Cons: Production crashes, poor error messages, unprofessional

**Option 2: Lint warning on .unwrap()** (warn but allow)
- Pros: Flexibility for "known safe" cases, can override
- Cons: Developers will override frequently, warning fatigue

**Option 3: Deny .unwrap() at compile time** (chosen)
```rust
#![deny(clippy::unwrap_used)]
```
- Pros: Forces proper error handling, prevents panics, professional code
- Cons: More verbose error handling required

## Decision

**Enforce `#![deny(clippy::unwrap_used)]` at crate root - `.unwrap()` calls will not compile.**

### Implementation

In `rust/bundlebase/src/lib.rs`:
```rust
#![deny(clippy::unwrap_used)]
```

This makes any `.unwrap()` call a compilation error, forcing developers to handle errors explicitly.

### Required Alternatives

Instead of `.unwrap()`, use:

**Option 1: `?` operator** (propagate error to caller)
```rust
// ❌ WRONG: Panic on None
let value = option.unwrap();

// ✅ CORRECT: Return error to caller
let value = option.ok_or_else(|| "value not found".into())?;
```

**Option 2: Pattern matching** (handle explicitly)
```rust
// ❌ WRONG
let config = load_config().unwrap();

// ✅ CORRECT: Explicit handling
match load_config() {
    Ok(config) => process(config),
    Err(e) => return Err(format!("Config load failed: {}", e).into()),
}
```

**Option 3: Default values** (when None is OK)
```rust
// ❌ WRONG
let count = map.get(&key).unwrap();

// ✅ CORRECT: Provide default
let count = map.get(&key).unwrap_or(&0);
```

**Option 4: `.expect()` with context** (ONLY for truly impossible cases)
```rust
// Use .expect() ONLY when failure is programmer error
let schema = RecordBatch::schema(&batch)
    .expect("BUG: RecordBatch must have schema");
```

Note: `.expect()` still panics, so use **very sparingly** and only for cases that "can't happen" (document why with `BUG:` or `INVARIANT:` prefix).

## Consequences

### Positive

- **No panics**: Prevents production crashes from unwrap()
- **Better error messages**: Force developers to provide context
- **Explicit handling**: Clear what happens on error paths
- **Professional code**: Library code handles all error cases
- **Type safety**: Compiler enforces error handling
- **Debugging**: Errors include context about what went wrong

### Negative

- **More verbose**: `option.ok_or_else(|| "error")` vs `option.unwrap()`
- **Learning curve**: New Rust developers need to learn error handling patterns
- **Temptation to workaround**: Developers might be tempted to use `.expect()` instead
- **Refactoring burden**: Changing `unwrap()` to proper handling takes time

### Neutral

- **Code review**: Reviewers must check that `.expect()` is justified (if used at all)
- **Error type conversions**: May need more `.map_err()` calls
- **Question mark operator**: Heavy use of `?` throughout codebase

## Implementation Patterns

### Converting Option to Result

```rust
// For configuration/setup
let value = option.ok_or_else(|| {
    BundlebaseError::from("Required configuration missing")
})?;

// For user input
let column = columns.get(&name).ok_or_else(|| {
    BundlebaseError::from(format!("Column not found: {}", name))
})?;
```

### Handling Results

```rust
// Propagate with context
let data = load_data(path)
    .map_err(|e| format!("Failed to load {}: {}", path, e))?;

// Provide fallback
let config = load_config().unwrap_or_else(|_| Config::default());

// Pattern match for complex handling
match operation.validate() {
    Ok(()) => continue_processing(),
    Err(ValidationError::MissingColumn(col)) => {
        return Err(format!("Column required: {}", col).into())
    }
    Err(ValidationError::TypeMismatch(expected, got)) => {
        return Err(format!("Expected {}, got {}", expected, got).into())
    }
}
```

### Converting to Python Errors

```rust
// PyO3 error conversion
#[pymethod]
fn attach(&mut self, path: String) -> PyResult<Self> {
    self.inner.attach(&path)
        .map_err(|e| PyErr::new::<PyRuntimeError, _>(
            format!("Failed to attach {}: {}", path, e)
        ))?;
    Ok(self.clone())
}
```

## Comparison with Standard Library

Rust's standard library uses `.unwrap()` sparingly:
- Examples and tests: Yes (controlled environment)
- Library code: Almost never (use `?` or `match`)
- Panic-based APIs: Only when documented (e.g., `Vec::remove` panics on out-of-bounds)

Bundlebase follows standard library conventions: proper error handling, no unwrap.

## Related Decisions

- [ADR-001](001-rust-core.md) - Rust enables compile-time error handling
- See [ai-rules.md section 1.3](../ai-rules.md) - Enforcement in AI code generation
- See [anti-patterns.md section 2](../anti-patterns.md) - `.unwrap()` anti-pattern examples
- See [errors.md](../errors.md) - Error handling patterns

## Exception: Test Code

Test code **may** use `.unwrap()` because panics in tests are acceptable:

```rust
#[test]
fn test_filter() {
    let container = Container::create("/tmp/test").unwrap(); // OK in tests
    container.filter("age > 18").unwrap(); // OK in tests
    assert_eq!(container.row_count(), 100);
}
```

Rationale: Tests should fail loudly, and unwrap provides clear failure point.

## Tooling

```bash
# Check for unwrap violations
cargo clippy

# Will fail with:
# error: used `unwrap()` on `Option` value
#   --> src/bundle.rs:123:18
#    |
# 123 |     let value = option.unwrap();
#    |                 ^^^^^^^^^^^^^^^
```

## Migration Strategy

For existing `.unwrap()` calls:

1. **Identify context**: Why is unwrap used here?
2. **Choose alternative**:
   - Can propagate? Use `?`
   - Need default? Use `.unwrap_or()`
   - Truly impossible? Use `.expect("BUG: why this can't fail")`
3. **Add error context**: Provide meaningful error message
4. **Test error path**: Ensure error is handled correctly

## References

- Rust Error Handling Book: https://doc.rust-lang.org/book/ch09-00-error-handling.html
- Rust API Guidelines (C-GOOD-ERR): https://rust-lang.github.io/api-guidelines/errors.html
