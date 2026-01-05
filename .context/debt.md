# Technical Debt

This document tracks known technical debt in bundlebase - shortcuts, workarounds, missing features, and areas needing improvement. Managing debt explicitly helps us make informed trade-offs between speed and quality.

## What is Technical Debt?

**Technical debt** is code that works but could be better. Like financial debt, it's sometimes strategic to take on debt to ship faster, but it must be tracked and paid down eventually.

**Examples of debt:**
- Workarounds instead of proper fixes
- Missing validation or error handling
- Performance optimizations not yet implemented
- Code duplication that should be abstracted
- Incomplete test coverage
- Documentation out of sync with code

**Not debt:** Bugs are not debt - they should be fixed immediately. Debt is code that works but isn't ideal.

---

## Debt Categories

### High Priority 🔴
**Impact:** Affects users, limits functionality, or creates risk

**Action:** Should be addressed in next 1-2 releases

### Medium Priority 🟡
**Impact:** Internal quality issues, minor user annoyance, or future constraints

**Action:** Address when convenient, within 3-6 months

### Low Priority 🟢
**Impact:** Nice-to-have improvements, optimization opportunities

**Action:** Address when time permits, no specific timeline

---

## Current Technical Debt

### Code Quality

#### 🟡 Incomplete Input Validation

**Issue:** Some operations don't validate all preconditions

**Impact:** Errors caught during execution instead of in `check()` phase

**Example:**
```rust
// Some operations validate minimally
fn check(&self, state: &BundleState) -> Result<()> {
    // Only checks column exists, doesn't validate types
    if !state.schema().column_with_name(&self.column).is_some() {
        return Err(...);
    }
    Ok(())
}

// Should also validate:
// - Column type is compatible with operation
// - Expression parses correctly
// - No circular dependencies
```

**Cost to fix:** Medium - need to audit all operations

**Benefit:** Better error messages, fail fast

**Related:** [decisions/006-lazy-evaluation.md](decisions/006-lazy-evaluation.md)

**Status:** Not started

---

#### 🟡 Error Message Consistency

**Issue:** Error messages vary in format and helpfulness

**Impact:** Inconsistent user experience, some errors hard to debug

**Example:**
```rust
// Various error formats across codebase
"Column not found"
"Column 'age' not found"
"Column 'age' not found. Available: name, status, id"
"Failed to find column 'age' in schema"
```

**Proposed solution:**
- Define standard error message format
- Always include context (what was being attempted)
- Always suggest fix when obvious
- List available options when applicable

**Cost to fix:** Low - mostly string changes

**Benefit:** Better user experience, easier debugging

**Related:** [errors.md](errors.md#error-message-guidelines)

**Status:** Not started

---

#### 🟢 Code Duplication in Python Wrappers

**Issue:** Similar boilerplate repeated across Python bindings

**Impact:** More code to maintain, changes need to be repeated

**Example:**
```python
# Pattern repeated in many methods:
async def method_a(self, ...) -> "AsyncContainer":
    await self._inner.method_a(...)
    return self

async def method_b(self, ...) -> "AsyncContainer":
    await self._inner.method_b(...)
    return self

# Could potentially use decorator or codegen
```

**Cost to fix:** Medium - requires refactoring

**Benefit:** Less boilerplate, easier to maintain

**Trade-off:** Might reduce clarity (explicit is better than magic)

**Status:** Deferred - not worth complexity of solution

---

### Performance

#### 🟡 Schema Mismatch Handling in UNIONs

**Issue:** Multi-source UNIONs with schema mismatches could be handled better

**Impact:** Some valid use cases fail or require manual schema alignment

**Current behavior:**
```rust
// If schemas don't match exactly, operation fails
container.attach("file1.parquet")  // Schema: [name, age, city]
container.attach("file2.parquet")  // Schema: [name, age, state]
// ❌ Fails: city vs state column mismatch
```

**Desired behavior:**
- Automatically align schemas (fill missing columns with nulls)
- Or at least provide clear error with schema diff
- Or provide utility to manually align schemas

**Cost to fix:** Medium - requires schema merge logic

**Benefit:** More flexible multi-source operations

**Related:** [architecture.md](architecture.md#adapters)

**Status:** Not started

---

#### 🟢 Row Indexing is Lazy

**Issue:** Row indices built on first use, not pre-computed

**Impact:** First index lookup slower than subsequent ones

**Current behavior:**
```python
# First call builds index (slow for large datasets)
row = c.get_row(1000000)  # ~500ms

# Subsequent calls use cached index (fast)
row = c.get_row(1000001)  # ~1ms
```

**Trade-off:**
- **Current (lazy):** No cost if index never used, first use slow
- **Alternative (eager):** Always pay build cost, first use fast

**Decision:** Keep lazy - most users don't use indexing

**Cost to fix:** Low - just build index during attach

**Benefit:** Faster first index lookup

**Status:** Deferred - lazy is correct default (see README.md)

---

### Testing

#### 🔴 Limited Large Dataset Testing

**Issue:** Most tests use small sample files (<100MB)

**Impact:** Streaming violations or memory issues might not be caught

**Current state:**
- Rust tests: mostly small datasets
- Python tests: mostly small datasets
- No automated tests with multi-GB files

**Proposed solution:**
- Add CI job that runs subset of tests with large files (5-10GB)
- Generate large test files programmatically (don't commit them)
- Mark tests with `@pytest.mark.large_data` for optional runs

**Cost to fix:** Medium - need test infrastructure

**Benefit:** Catch streaming violations, verify performance claims

**Related:** [decisions/003-streaming-only.md](decisions/003-streaming-only.md), [testing.md](testing.md)

**Status:** Planned for v0.3.0

---

#### 🟡 Incomplete Error Case Coverage

**Issue:** Not all error paths have tests

**Impact:** Some error cases might fail ungracefully

**Examples of missing tests:**
- Corrupt Parquet files
- Operations on empty datasets
- Very long column names (>1000 chars)
- Unicode in SQL expressions
- Deeply nested directory structures

**Cost to fix:** Medium - need to write tests for each case

**Benefit:** More robust error handling

**Status:** Ongoing - add as we encounter issues

---

### Documentation

#### 🟡 Examples in Docs May Be Outdated

**Issue:** Some code examples in documentation haven't been tested recently

**Impact:** Users might copy examples that don't work

**Proposed solution:**
- Extract examples into actual test files
- Use `doctest` in Rust
- Use `doctest` in Python
- Run examples as part of CI

**Cost to fix:** Medium - need to set up infrastructure

**Benefit:** Examples always work, better user experience

**Related:** [testing.md](testing.md)

**Status:** Not started

---

#### 🟢 Missing Performance Benchmarks

**Issue:** No formal benchmark suite

**Impact:** Can't track performance regressions over time

**Proposed solution:**
- Add `cargo bench` benchmarks for critical paths
- Add Python benchmarks using `pytest-benchmark`
- Track results over time (e.g., in CI)

**Cost to fix:** Medium - need to write benchmarks

**Benefit:** Catch performance regressions, prove performance claims

**Related:** [performance.md](performance.md)

**Status:** Deferred - not critical for alpha

---

### Architecture

#### 🔴 Missing Features (Planned)

**Issue:** Core features not yet implemented

**Impact:** Users can't accomplish some common tasks

**Missing features:**
- **Joins** - No way to join multiple containers
- **Aggregations** - No built-in group by / aggregate
- **Window functions** - No support for window operations
- **Data modification** - No UPDATE/DELETE operations
- **Custom UDFs** - Limited ability to add custom functions

**Status:** Roadmap items for future versions

**Priority varies:**
- Joins: 🔴 High priority (v0.3.0 planned)
- Aggregations: 🔴 High priority (v0.3.0 planned)
- Window functions: 🟡 Medium priority (v0.4.0)
- Data modification: 🟢 Low priority (read-only is OK for now)
- Custom UDFs: 🟡 Medium priority (v0.4.0)

---

#### 🟡 Function Registry is Simplified

**Issue:** Current function system is basic, doesn't support all use cases

**Limitations:**
- Can't validate function signatures at registration time
- No type inference for function outputs
- Limited error messages for function failures
- No function categories or namespacing

**Impact:** Functions work but could be more robust

**Cost to fix:** High - significant architecture change

**Benefit:** More robust, better error messages, more features

**Status:** Deferred to v0.4.0 (current system sufficient for now)

---

### Dependencies

#### 🟢 DataFusion Version Lock

**Issue:** Locked to DataFusion v51, haven't upgraded to v52+

**Impact:** Missing new DataFusion features and optimizations

**Reason for debt:** API changes require code updates, testing

**Cost to fix:** Medium - need to update API usage, test thoroughly

**Benefit:** New features, bug fixes, performance improvements

**Related:** [dependencies.md](dependencies.md)

**Status:** Planned for v0.3.0 after testing

---

## Debt Mitigation Strategies

### Prevention

**How to avoid creating debt:**

1. **Don't skip validation** - Always validate in `check()` phase
2. **Write tests first** - TDD prevents shortcuts
3. **Document trade-offs** - If taking shortcut, explain why in comment
4. **Review carefully** - Catch debt in code review
5. **Track in debt.md** - If you know it's debt, document it

**When debt is OK:**
- Shipping MVP quickly (document debt for later)
- Performance optimization can wait until proven necessary
- Feature not used yet (YAGNI - You Aren't Gonna Need It)

**When debt is NOT OK:**
- Violates critical constraints (streaming, no unwrap, etc.)
- Creates security vulnerability
- Causes data corruption
- Makes future changes difficult

---

### Paying Down Debt

**Prioritization:**
1. **High priority first** - Affects users or limits functionality
2. **Quick wins** - Low effort, high benefit items
3. **Cluster related items** - Fix all error messages at once
4. **Before adding features** - Don't build on shaky foundation

**Process:**
1. Pick debt item from this document
2. Create GitHub issue referencing this doc
3. Implement fix
4. Add tests to prevent regression
5. Update this document (mark as resolved)

**Budget:**
- Allocate ~20% of time to debt paydown
- Don't let debt grow unbounded
- Major refactors should be planned releases

---

## Debt Review Process

### Quarterly Review

Every quarter, review this document:

1. **Triage new items** - Assign priority (high/medium/low)
2. **Promote priorities** - Medium → High if impact grew
3. **Close resolved items** - Move to "Resolved Debt" section
4. **Assess burden** - Too much high-priority debt? Slow down features.

### Adding New Debt

When you discover or create debt:

1. **Document it** - Add to this file with details:
   - What is the debt?
   - Why does it exist?
   - What's the impact?
   - How hard to fix?
   - Priority (high/medium/low)
   - Status (not started/in progress/deferred)

2. **Link to code** - Reference files, functions, line numbers

3. **Explain trade-off** - Why this shortcut made sense

**Example template:**
```markdown
#### 🟡 Brief Title

**Issue:** Clear description of the problem

**Impact:** How this affects users or development

**Example:**
[Code example showing the debt]

**Cost to fix:** Low/Medium/High - brief explanation

**Benefit:** What we gain by fixing

**Related:** [link to relevant docs]

**Status:** Not started / In progress / Deferred
```

---

## Resolved Debt

Track resolved items for historical context:

### ✅ Removed All `.unwrap()` Calls (v0.2.0)

**Was:** Code used `.unwrap()` liberally, causing panics

**Fixed:** Added `#![deny(clippy::unwrap_used)]`, refactored all unwrap calls

**Benefit:** No more panics, better error messages

**See:** [decisions/007-no-unwrap.md](decisions/007-no-unwrap.md)

---

### ✅ Replaced All `collect()` with Streaming (v0.1.5)

**Was:** Some code paths used `collect()`, loading entire datasets

**Fixed:** Refactored to use `execute_stream()` throughout

**Benefit:** Constant memory usage, handle datasets larger than RAM

**See:** [decisions/003-streaming-only.md](decisions/003-streaming-only.md)

---

### ✅ Unified Python API (v0.2.0)

**Was:** Mixed async/sync APIs, confusing for users

**Fixed:** Clear separation: `AsyncContainer` for async, `Container` for sync

**Benefit:** Consistent API, works in both Jupyter and async contexts

**See:** [sync-api.md](sync-api.md)

---

## Debt Metrics

Track debt over time:

| Version | High Priority | Medium Priority | Low Priority | Total |
|---------|---------------|-----------------|--------------|-------|
| v0.2.1  | 2             | 4               | 4            | 10    |
| v0.2.0  | 3             | 5               | 3            | 11    |
| v0.1.5  | 5             | 4               | 2            | 11    |

**Trend:** Debt stable (adding and resolving at similar rate)

**Goal:** Keep high-priority debt under 5 items

---

## Debt Philosophy

### Technical Debt is Normal

**All software has debt.** The key is managing it consciously:

- **Accept debt strategically** - Speed vs. quality trade-offs are real
- **Track debt explicitly** - Don't let it hide in code comments
- **Pay down regularly** - Don't let debt accumulate unbounded
- **Prioritize ruthlessly** - Fix high-impact debt first

### When to Take on Debt

**Good reasons:**
- Ship MVP faster (with plan to fix later)
- Optimize only proven bottlenecks (avoid premature optimization)
- YAGNI - Don't build features you might not need

**Bad reasons:**
- "No time to do it right" (always makes time later)
- "It works, ship it" (without documenting debt)
- Ignoring critical constraints (streaming, error handling)

### Debt vs. Quality

Debt is not an excuse for low quality:

**Never acceptable as debt:**
- Violating critical constraints (no unwrap, no collect)
- Security vulnerabilities
- Data corruption bugs
- Completely missing error handling

**Acceptable as tracked debt:**
- Incomplete validation (basic checks work, comprehensive later)
- Suboptimal performance (works, optimize when proven necessary)
- Code duplication (works, refactor when pattern clear)
- Missing edge case tests (main path tested, edge cases later)

---

## Related Documentation

- [anti-patterns.md](anti-patterns.md) - What NOT to do (creates debt)
- [code-review.md](code-review.md) - Catch debt in review
- [decisions/](decisions/) - ADRs explain why some "debt" is actually intentional
- [workflows.md](workflows.md) - How to fix debt systematically

---

## Summary

**Current state:** 10 debt items (2 high, 4 medium, 4 low)

**High priority items:**
1. Limited large dataset testing
2. Missing core features (joins, aggregations)

**Next quarter focus:**
- Add large dataset tests (prevents streaming violations)
- Plan joins feature (most requested)

**Philosophy:** Debt is tracked, prioritized, and paid down regularly. We accept strategic debt but never sacrifice critical constraints.

---

**Last Updated:** January 2026
**Next Review:** April 2026
