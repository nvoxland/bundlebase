# Code Review Guidelines

This document defines bundlebase's code review process, checklists, and standards. Code review is critical for maintaining quality, catching bugs early, and ensuring architectural consistency.

## Code Review Philosophy

1. **Catch bugs before merge** - Reviews are the last line of defense
2. **Enforce constraints** - Ensure critical rules (streaming, no unwrap, etc.) are followed
3. **Knowledge sharing** - Reviews help team understand codebase changes
4. **Maintain quality** - Consistent standards across all code
5. **Be constructive** - Reviews should help, not criticize

**Key principle:** All code must be reviewed before merging to main.

---

## Pre-Review Checklist (Author)

Before requesting review, ensure:

### Code Quality

- [ ] Code compiles without errors (`cargo build`)
- [ ] No clippy warnings (`cargo clippy`)
- [ ] All tests pass (`cargo test` and `poetry run pytest`)
- [ ] Code formatted (`cargo fmt`)
- [ ] No debug code (print statements, commented code, TODOs without issues)

### Critical Constraints

- [ ] **No `.unwrap()` or `.expect()` calls** (except in tests)
- [ ] **No `collect()` calls** on DataFrames (use `execute_stream()`)
- [ ] **No `mod.rs` files** (use named module files like `module.rs`)
- [ ] Operations return `&mut Self` for chaining (Rust) or `self` (Python)
- [ ] Proper error handling with context (no generic error messages)

### Testing

- [ ] New functionality has tests (Rust unit tests or Python E2E tests)
- [ ] Tests cover success cases
- [ ] Tests cover error cases
- [ ] Tests pass with large datasets (if data processing code)
- [ ] Performance not regressed (memory usage constant for streaming)

### Documentation

- [ ] Public Rust items have doc comments (`///`)
- [ ] Python methods have docstrings (with Args, Returns, Raises, Example)
- [ ] Type hints on all Python parameters and returns
- [ ] Relevant `.context/` files updated (if architectural change)
- [ ] ADR created (if major architectural decision)

### Git Hygiene

- [ ] Commits are logical and well-described
- [ ] Commit messages follow convention (e.g., "feat:", "fix:", "docs:")
- [ ] No merge commits (rebase instead)
- [ ] PR description explains what and why (not just what)

---

## Review Checklist (Reviewer)

When reviewing code, check these items systematically:

### 1. Critical Constraints ⚠️

**These are BLOCKING issues - code cannot merge if violated.**

#### Streaming Execution

- [ ] **No `collect()` calls** - Search PR for `.collect()` in Rust code
  ```rust
  // ❌ BLOCKING
  let batches = df.collect().await?;

  // ✅ CORRECT
  let mut stream = df.execute_stream().await?;
  ```

- [ ] **Verify streaming in Python** - Python methods should use Rust streaming internally
  ```python
  # ❌ BLOCKING - accumulating batches
  batches = [batch for batch in self.stream_batches()]

  # ✅ CORRECT - let Rust handle streaming
  return self._inner.to_pandas()
  ```

**See:** [decisions/003-streaming-only.md](decisions/003-streaming-only.md)

#### Error Handling

- [ ] **No `.unwrap()` calls** - Search PR for `.unwrap()`
  ```rust
  // ❌ BLOCKING
  let value = option.unwrap();

  // ✅ CORRECT
  let value = option.ok_or_else(|| error)?;
  ```

- [ ] **Errors have context** - Error messages include what failed
  ```rust
  // ❌ BAD - generic error
  return Err("failed".into());

  // ✅ GOOD - specific context
  return Err(format!("Failed to read '{}': {}", path, e).into());
  ```

**See:** [decisions/007-no-unwrap.md](decisions/007-no-unwrap.md), [errors.md](errors.md)

#### Module Organization

- [ ] **No `mod.rs` files** - Check that new modules use named files
  ```
  ❌ BLOCKING: src/feature/mod.rs
  ✅ CORRECT: src/feature.rs
  ```

**See:** [decisions/008-no-mod-rs.md](decisions/008-no-mod-rs.md)

#### Operation Pattern

If PR adds or modifies operations:

- [ ] **Three-phase pattern** - `check()`, `reconfigure()`, `apply_dataframe()` all implemented
- [ ] **Validation in check()** - Errors caught before execution
- [ ] **Schema updated in reconfigure()** - If operation changes schema
- [ ] **Streaming in apply_dataframe()** - Uses DataFusion, no collect()

**See:** [decisions/006-lazy-evaluation.md](decisions/006-lazy-evaluation.md)

#### Python Bindings

If PR adds Python bindings:

- [ ] **Arc clones returned** - PyO3 methods return `self.clone()`, not `&mut self`
  ```rust
  // ❌ BLOCKING - can't return Rust ref to Python
  fn filter(&mut self) -> PyResult<&mut Self>

  // ✅ CORRECT - clone Arc (cheap)
  fn filter(&mut self) -> PyResult<Self> { Ok(self.clone()) }
  ```

- [ ] **Both async and sync APIs** - Both `AsyncContainer` and `Container` updated
- [ ] **Methods return self** - Enable chaining in Python

**See:** [python-bindings.md](python-bindings.md)

---

### 2. Code Quality

#### Correctness

- [ ] Logic is correct (no off-by-one, edge cases handled)
- [ ] Thread safety considered (if concurrent access)
- [ ] No race conditions (if async code)
- [ ] Resources cleaned up (files closed, connections released)

#### Performance

- [ ] No unnecessary clones of large data
- [ ] Efficient algorithms (not O(n²) where O(n) possible)
- [ ] Memory usage reasonable (constant for streaming code)
- [ ] No blocking operations in async code

**See:** [performance.md](performance.md)

#### Security

- [ ] User input validated (SQL expressions parsed, paths checked)
- [ ] No SQL injection possible (use parameterized queries)
- [ ] No path traversal vulnerabilities (validate file paths)
- [ ] Secrets not logged or committed

**See:** [boundaries.md](boundaries.md#trust-boundaries)

---

### 3. Testing

#### Test Coverage

- [ ] New functionality has tests
- [ ] Tests are in correct location:
  - Rust unit tests: Same file as code (`#[cfg(test)]`)
  - Rust integration tests: `tests/` directory
  - Python E2E tests: `python/tests/`
- [ ] Tests cover happy path (success case)
- [ ] Tests cover error cases (invalid input, missing files, etc.)
- [ ] Tests use sample data (not production data)

**See:** [testing.md](testing.md)

#### Test Quality

- [ ] Test names are descriptive (`test_filter_removes_rows`)
- [ ] Tests are independent (can run in any order)
- [ ] Tests clean up after themselves (temp files removed)
- [ ] Async tests use `#[tokio::test]` (Rust) or `@pytest.mark.asyncio` (Python)

#### Performance Tests

If PR modifies data processing:

- [ ] Test with large dataset (>1GB if possible)
- [ ] Memory usage verified (should be constant)
- [ ] No collect() calls introduced

---

### 4. Documentation

#### Code Documentation

- [ ] Public Rust items have doc comments (`///`)
  ```rust
  /// Filters rows based on SQL WHERE expression.
  ///
  /// # Arguments
  /// * `expr` - SQL expression (without WHERE keyword)
  ///
  /// # Example
  /// ```no_run
  /// container.filter("age >= 18")?;
  /// ```
  pub fn filter(&mut self, expr: &str) -> Result<&mut Self>
  ```

- [ ] Python methods have docstrings
  ```python
  def filter(self, expression: str) -> "Container":
      """Filter rows based on SQL WHERE clause.

      Args:
          expression: SQL WHERE expression

      Returns:
          Self for method chaining

      Raises:
          ValueError: If expression is invalid

      Example:
          >>> c.filter("age >= 18")
      """
  ```

- [ ] Type hints on all Python parameters and returns
- [ ] Complex logic has explanatory comments

#### Project Documentation

- [ ] Relevant `.context/` files updated (if architectural change)
- [ ] ADR created if major decision (see [decisions/README.md](decisions/README.md))
- [ ] CLAUDE.md updated if new feature changes workflow
- [ ] Examples in docs are runnable and current

---

### 5. API Design

#### Consistency

- [ ] Naming follows conventions:
  - Rust: `snake_case` for functions, `PascalCase` for types
  - Python: `snake_case` for everything
- [ ] Similar functions have similar signatures
- [ ] Follows existing patterns (don't reinvent the wheel)

#### Usability

- [ ] API is intuitive (obvious how to use)
- [ ] Error messages are actionable (user knows what to fix)
- [ ] Method chaining works (returns `&mut self` or `self`)
- [ ] Default values sensible (common case works without config)

#### Compatibility

- [ ] No breaking changes to public API (unless major version bump)
- [ ] Deprecation warnings if removing functionality
- [ ] Migration guide if breaking change necessary

---

### 6. Specific File Type Reviews

#### Reviewing Operations (`bundle/operations/*.rs`)

- [ ] Implements `Operation` trait fully
- [ ] `check()` validates without executing
- [ ] `reconfigure()` updates schema/state
- [ ] `apply_dataframe()` uses streaming
- [ ] Derives `Clone, Serialize, Deserialize` (for manifest support)
- [ ] Has Rust test and Python E2E test

**Template:** [prompts/add-operation.md](prompts/add-operation.md)

#### Reviewing Python Bindings (`python/bundlebase/src/*.rs`, `python/bundlebase/*.py`)

- [ ] PyO3 wrapper returns `self.clone()` (Rust side)
- [ ] Both `AsyncContainer` and `Container` updated (Python side)
- [ ] Type hints complete
- [ ] Docstrings complete with examples
- [ ] Errors mapped to appropriate Python exceptions

**Template:** [prompts/add-python-binding.md](prompts/add-python-binding.md)

#### Reviewing Tests (`tests/`, `python/tests/`)

- [ ] Test naming clear (`test_<functionality>_<scenario>`)
- [ ] Assertions meaningful (`assert x == expected`, not just `assert x`)
- [ ] Error messages in assertions (e.g., `assert x == y, f"Expected {y}, got {x}"`)
- [ ] Tests don't rely on timing (no `sleep()` unless necessary)
- [ ] Temp directories cleaned up

---

## Common Issues to Flag

### Performance Issues

**Memory growth with dataset size:**
```rust
// ❌ FLAG THIS
let batches = df.collect().await?;  // Memory grows with data
```

**Unnecessary clones:**
```rust
// ❌ FLAG THIS
let copy = large_dataframe.clone();  // Expensive if not needed
```

**Multiple passes over data:**
```rust
// ❌ FLAG THIS - two passes
let count = df.clone().count().await?;
let batches = df.collect().await?;

// ✅ SUGGEST THIS - single pass
let mut stream = df.execute_stream().await?;
let mut count = 0;
while let Some(batch) = stream.next().await {
    count += batch?.num_rows();
}
```

---

### Error Handling Issues

**Generic errors:**
```rust
// ❌ FLAG THIS
return Err("failed".into());

// ✅ SUGGEST THIS
return Err(format!("Failed to parse SQL expression '{}': {}", expr, e).into());
```

**Swallowing errors:**
```rust
// ❌ FLAG THIS
let _ = operation.check(state);  // Ignores errors!

// ✅ SUGGEST THIS
operation.check(state)?;
```

---

### API Design Issues

**Inconsistent naming:**
```rust
// ❌ FLAG THIS - mixed conventions
pub fn getData() -> Result<DataFrame>  // camelCase in Rust

// ✅ SUGGEST THIS
pub fn get_data() -> Result<DataFrame>  // snake_case
```

**Unclear errors:**
```python
# ❌ FLAG THIS
raise RuntimeError("Error")

# ✅ SUGGEST THIS
raise ValueError(f"Column '{name}' not found. Available: {available_columns}")
```

---

## Review Process

### 1. Initial Review

1. **Read PR description** - Understand what and why
2. **Check CI status** - All tests passing?
3. **Review commits** - Logical progression?
4. **Scan changes** - Get overall sense of changes

### 2. Detailed Review

1. **Run checklist** - Go through checklist above systematically
2. **Check critical constraints first** - Blocking issues?
3. **Review code quality** - Logic, performance, security
4. **Verify tests** - Coverage adequate?
5. **Check documentation** - Complete and accurate?

### 3. Provide Feedback

**For blocking issues:**
```
🚨 BLOCKING: Using .collect() on line 42 violates streaming constraint.

Replace with:
let mut stream = df.execute_stream().await?;
while let Some(batch) = stream.next().await { ... }

See: .context/decisions/003-streaming-only.md
```

**For suggestions:**
```
💡 SUGGESTION: Consider adding error context here.

Instead of:
return Err(e)

Consider:
return Err(format!("Failed to load manifest from '{}': {}", path, e).into())
```

**For questions:**
```
❓ QUESTION: Why is this clone necessary? Could we use a reference instead?
```

**For praise:**
```
✅ NICE: Good error message with context and available columns listed!
```

### 4. Approve or Request Changes

- **Approve** - If all checks pass and no blocking issues
- **Request Changes** - If blocking issues (streaming violation, unwrap, etc.)
- **Comment** - If only suggestions/questions

---

## Self-Review Checklist

Before submitting PR, author should self-review:

1. **Read your own code** - As if you're the reviewer
2. **Run the checklists** - Pre-review and review checklists
3. **Check diffs** - No unintended changes (debug code, formatting)
4. **Test locally** - All tests pass on your machine
5. **Review documentation** - Docstrings accurate, examples work

**Tip:** Review your code in GitHub's PR view, not just your editor. Seeing it formatted differently helps catch issues.

---

## Handling Disagreements

If author and reviewer disagree:

1. **Discuss rationale** - Both explain reasoning
2. **Consult documentation** - Check `.context/` files, ADRs
3. **Ask for second opinion** - Get another reviewer
4. **Defer to constraints** - If violates hard rule, rule wins
5. **Document decision** - Update ADR or anti-patterns if needed

**Remember:** Constraints like "no unwrap" and "streaming only" are non-negotiable.

---

## Review Time Expectations

**Author expectations:**
- Code ready for review (all checklist items complete)
- Respond to feedback within 1 day
- Address blocking issues before requesting re-review

**Reviewer expectations:**
- Initial review within 1 day (blocking issues flagged)
- Detailed review within 2 days
- Re-review within 1 day (after author addresses feedback)

**For urgent PRs:**
- Tag reviewer explicitly
- Explain urgency in PR description
- Consider pair review for fastest turnaround

---

## Review Tools

### Automated Checks (CI)

These run automatically:
- Rust compilation (`cargo build`)
- Rust tests (`cargo test`)
- Clippy lints (`cargo clippy`)
- Python tests (`poetry run pytest`)
- Formatting (`cargo fmt --check`)

**Reviewer:** If CI fails, don't review until fixed.

### Manual Checks

These require human review:
- Streaming constraint (no collect())
- Error handling (no unwrap())
- API design (consistency, usability)
- Documentation quality
- Test coverage

### Search Patterns

Quick searches to find common issues:

```bash
# Find collect() calls (should be rare)
rg "\.collect\(\)" --type rust

# Find unwrap() calls (should only be in tests)
rg "\.unwrap\(\)" --type rust

# Find mod.rs files (shouldn't exist)
find . -name "mod.rs"

# Find TODO comments (should have issue numbers)
rg "TODO" --type rust
```

---

## Examples

### Example 1: Good Review Comment

```markdown
🚨 BLOCKING: Streaming violation on line 127

You're using `df.collect().await?` which loads the entire dataset into memory.
This violates our streaming-only constraint and will fail for large datasets.

Replace with streaming execution:
```rust
let mut stream = df.execute_stream().await?;
while let Some(batch) = stream.next().await {
    let batch = batch?;
    // Process batch incrementally
}
```

See: .context/decisions/003-streaming-only.md for rationale.
```

### Example 2: Good Review Comment (Suggestion)

```markdown
💡 SUGGESTION: Add error context

The error message on line 54 could be more helpful:
```rust
// Current
.map_err(|e| e.into())?;

// Suggested
.map_err(|e| format!("Failed to parse SQL expression '{}': {}", expr, e).into())?;
```

This helps users understand what went wrong and what they need to fix.

See: .context/errors.md#error-message-guidelines
```

### Example 3: Good Review Comment (Praise)

```markdown
✅ NICE: Excellent test coverage!

I love that you tested both success case (line 234) and error cases (lines 245-260).
The test names are descriptive and the assertions include helpful messages.
```

---

## Summary

**Critical review points (BLOCKING if violated):**
1. ⚠️ No `collect()` calls on DataFrames
2. ⚠️ No `.unwrap()` or `.expect()` calls
3. ⚠️ No `mod.rs` files
4. ⚠️ Operations implement three-phase pattern
5. ⚠️ Python bindings return Arc clones
6. ⚠️ Tests pass and cover new functionality

**Important review points:**
- Error handling with context
- Performance (constant memory, efficient algorithms)
- Documentation (docstrings, type hints, examples)
- API consistency and usability
- Security (input validation, no secrets)

**Review process:**
1. Check critical constraints first
2. Review code quality and tests
3. Verify documentation
4. Provide constructive feedback
5. Approve or request changes

**Remember:** Code review is about maintaining quality and sharing knowledge, not finding fault. Be constructive, specific, and kind.

---

## Related Documentation

- [ai-rules.md](ai-rules.md) - Hard constraints enforced in reviews
- [anti-patterns.md](anti-patterns.md) - Common issues to flag
- [testing.md](testing.md) - Testing requirements
- [decisions/](decisions/) - ADRs explaining why rules exist
- [prompts/](prompts/) - Templates for common development tasks

---

**Last Updated:** January 2026
