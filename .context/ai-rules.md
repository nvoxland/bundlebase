# AI Rules - Hard Constraints

This document contains **non-negotiable** patterns and constraints for AI code generation in the Bundlebase project. These rules override any conflicting guidance and must be followed strictly.

## 1. Development Workflow Constraints

### 1.1 Rust-First Development

**Rule**: Always implement in Rust first, then create Python bindings.

**Process**:
1. Write Rust implementation with full functionality
2. Add comprehensive Rust unit tests
3. Ensure all Rust tests pass (`cargo test`)
4. Create PyO3 bindings in `rust/bundlebase-python/src/`
5. Write E2E Python tests in `python/tests/`
6. Verify both test suites pass

**Rationale**: Core logic lives in Rust for performance and type safety. Python bindings are a thin wrapper.

**See**: [development.md](development.md) for detailed workflow

### 1.2 Never Use collect() - Always Stream (CRITICAL)

**Rule**: ALWAYS use `execute_stream()` for query execution. NEVER use `collect()`.

This is the **most important performance constraint** in the entire codebase.

#### Why This Matters

| Approach | 10GB File Memory Usage | Explanation |
|----------|----------------------|-------------|
| `collect()` | ~30GB peak RAM | Materializes entire dataset (3x size) |
| `execute_stream()` | ~50MB peak RAM | Processes in constant-size batches |

#### Rust Code Patterns

```rust
// ✅ CORRECT: Streaming execution
let stream = dataframe.execute_stream().await?;
let py_stream = PyRecordBatchStream::new(stream, schema);
return Ok(py_stream.into_py(py));

// ❌ WRONG: Materializes entire dataset
let batches = dataframe.collect().await?;  // NEVER DO THIS
for batch in batches {
    // Processing here happens after full materialization
}
```

#### Python Binding Patterns

```rust
// ✅ CORRECT: Return streaming object to Python
#[pyo3(name = "as_pyarrow_stream")]
fn as_pyarrow_stream<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
    let stream = self.execute_stream().await?;  // Stream, don't collect
    // Return stream to Python
}

// ❌ WRONG: Materialize before returning
#[pyo3(name = "as_pyarrow")]
fn as_pyarrow<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
    let batches = self.collect().await?;  // Full materialization
    // This defeats streaming and causes OOM on large data
}
```

#### Python Code Patterns

```python
# ✅ CORRECT: Stream batches incrementally
async def to_custom_format(container):
    results = []
    async for batch in stream_batches(container):
        chunk = process_batch(batch)  # Process one batch at a time
        results.append(chunk)
    return combine(results)

# ❌ WRONG: Accumulate all batches in memory
async def to_custom_format(container):
    arrow_table = await _get_arrow_table(container)  # Full load
    return convert(arrow_table)  # Entire dataset in RAM
```

#### Code Review Checklist

Before any commit, verify:
- [ ] No `collect()` calls on DataFrames
- [ ] No `Vec<RecordBatch>` accumulation
- [ ] Returns streaming objects to Python
- [ ] Python code uses `stream_batches()` or auto-streaming methods

**See**: [development.md lines 102-169](development.md), [anti-patterns.md](anti-patterns.md)

### 1.3 No Unwrap in Rust Code

**Rule**: The codebase enforces `#![deny(clippy::unwrap_used)]` at the crate root.

**Enforcement**: Line 1 of `rust/bundlebase/src/lib.rs`:
```rust
#![deny(clippy::unwrap_used)]
```

This means **any `.unwrap()` call will cause a compile error**.

#### Correct Error Handling

```rust
// ✅ CORRECT: Use ? operator
let value = option.ok_or_else(|| BundlebaseError::from("missing value"))?;
let result = fallible_operation()?;

// ✅ CORRECT: Use pattern matching
match option {
    Some(val) => process(val),
    None => return Err("value not found".into()),
}

// ✅ CORRECT: Use .map_err() for conversion
let result = rust_function()
    .map_err(|e| PyErr::new::<PyRuntimeError, _>(format!("Error: {}", e)))?;

// ❌ WRONG: Will not compile
let value = option.unwrap();  // Compile error!
let result = operation().unwrap();  // Compile error!
```

**Rationale**: Forces explicit error handling, prevents panics in production

**See**: `rust/bundlebase/src/lib.rs:1`

### 1.4 Git Commit Policy

**Rule**: ONLY create git commits when explicitly requested by the user.

**Rationale**: Automated commits without user approval are intrusive. Always ask first or wait for explicit instruction.

**See**: [development.md lines 202-207](development.md)

## 2. Architecture Constraints

### 2.1 Three-Tier Immutability

**Rule**: Respect the three-tier architecture and mutability semantics.

#### The Three Types

| Type | Mutability | Usage | Methods |
|------|------------|-------|---------|
| **Bundle trait** | Interface only | Common operations across all containers | Shared trait methods |
| **BundleBuilder** | **Mutable** | Building/transforming data | `&mut self` operations |
| **Bundle (read-only)** | **Immutable** | Loaded from commits | `&self` operations only |

#### Code Patterns

```rust
// ✅ CORRECT: Mutable operations on BundleBuilder
impl BundleBuilder {
    pub fn filter(&mut self, expr: &str) -> &mut Self {
        // Mutate in place
        self.operations.push(Operation::Filter(expr.to_string()));
        self  // Return &mut Self for chaining
    }
}

// ✅ CORRECT: Read-only operations on Bundle
impl Bundle {
    pub fn schema(&self) -> &Schema {
        &self.schema  // No mutation
    }
}

// ❌ WRONG: Trying to mutate immutable Bundle
impl Bundle {
    pub fn filter(&mut self, expr: &str) -> &mut Self {
        // Type error: Bundle is immutable!
    }
}
```

**Rationale**: Clear separation between mutable building and immutable querying prevents accidental mutations

**See**: [architecture.md](architecture.md)

### 2.2 Mutable Operations Return &mut Self

**Rule**: All mutation operations return `&mut Self` for fluent chaining.

```rust
// ✅ CORRECT: Chainable API
impl BundleBuilder {
    pub fn filter(&mut self, expr: &str) -> &mut Self {
        self.operations.push(Operation::Filter(expr.to_string()));
        self
    }

    pub fn select(&mut self, columns: Vec<String>) -> &mut Self {
        self.operations.push(Operation::Select(columns));
        self
    }
}

// Usage: Fluent chaining
let container = builder
    .filter("age >= 18")
    .select(vec!["name", "email"])
    .remove_column("temp");

// ❌ WRONG: Returns () - breaks chaining
impl BundleBuilder {
    pub fn filter(&mut self, expr: &str) {
        self.operations.push(Operation::Filter(expr.to_string()));
        // No return value
    }
}
```

**Rationale**: Enables fluent API matching Python's chaining style

**See**: [architecture.md](architecture.md), [python-api.md](python-api.md)

### 2.3 Lazy Evaluation

**Rule**: Operations are recorded when called, executed only during query time.

#### How It Works

```python
# These operations are RECORDED, not executed
c = c.filter("age >= 18")    # Adds FilterBlock to operations list
c = c.select(["name"])       # Adds SelectColumns to operations list

# THIS executes all recorded operations
df = await c.to_pandas()     # NOW queries run
```

#### Implementation Pattern

```rust
// ✅ CORRECT: Record operations
pub fn filter(&mut self, expr: &str) -> &mut Self {
    // Just record the operation
    self.operations.push(Operation::Filter(expr.to_string()));
    self  // No execution yet
}

// Execution happens here
async fn execute(&self) -> Result<DataFrame> {
    let mut df = self.initial_dataframe.clone();

    // Apply all recorded operations
    for op in &self.operations {
        df = op.apply(df).await?;
    }

    Ok(df)
}

// ❌ WRONG: Execute immediately
pub fn filter(&mut self, expr: &str) -> &mut Self {
    // Applying to dataframe immediately defeats lazy evaluation
    self.dataframe = self.dataframe.filter(expr).await.unwrap();
    self
}
```

**Rationale**: Enables query optimization, composable transformations

**See**: [architecture.md](architecture.md)

## 3. Python Binding Constraints

### 3.1 PyO3 Code Only in python/ Module

**Rule**: PyO3 code MUST only appear in `rust/bundlebase-python/src/` directory.

```
✅ rust/bundlebase-python/src/lib.rs       - PyO3 bindings
✅ rust/bundlebase-python/src/bundle.rs    - PyO3 wrappers
❌ rust/bundlebase/src/lib.rs              - NO PyO3 here
❌ rust/bundlebase/src/bundle.rs           - NO PyO3 here
```

**Rationale**: Keeps core Rust logic independent of Python bindings, enables other language bindings in future

**See**: [overview.md line 110](overview.md), [python-bindings.md](python-bindings.md)

### 3.2 Async Method Registration

**Rule**: Python methods must be registered in `python/src/bundlebase/__init__.py` to support both async and sync APIs.

#### Registration Pattern

```python
# python/src/bundlebase/__init__.py

# Register method on both Bundle and BundleBuilder classes
for cls in [Bundle, BundleBuilder]:
    # Async method
    register_async_method(cls, 'to_pandas', 'to_pandas')

    # Sync wrapper (for bundlebase.sync module)
    register_sync_method(cls, 'to_pandas', 'to_pandas')
```

#### Why This Matters

Without registration:
- Method won't work in `bundlebase.sync` module
- Method can't be chained fluently
- Async/sync bridge breaks

**See**: [sync-api.md](sync-api.md), `python/src/bundlebase/__init__.py`

### 3.3 Error Conversion Pattern

**Rule**: Convert Rust errors to Python exceptions using `.map_err()`.

```rust
// ✅ CORRECT: Error conversion
#[pymethod]
fn filter(&mut self, expr: String) -> PyResult<()> {
    self.inner
        .filter(&expr)
        .map_err(|e| PyErr::new::<PyRuntimeError, _>(format!("Filter error: {}", e)))?;
    Ok(())
}

// ❌ WRONG: Rust error propagates directly (type error)
#[pymethod]
fn filter(&mut self, expr: String) -> Result<(), BundlebaseError> {
    self.inner.filter(&expr)?;  // Wrong error type for PyO3
    Ok(())
}
```

**Standard Error Types**:
- `PyRuntimeError` - General runtime errors
- `PyValueError` - Invalid input values
- `PyIOError` - File I/O errors
- `PyTypeError` - Type mismatches

**See**: [python-bindings.md](python-bindings.md)

## 4. File Organization Constraints

### 4.1 No mod.rs Files

**Rule**: Never create `mod.rs` files. Use named module files instead.

```
✅ src/bundle.rs           - Module defined in own file
✅ src/operations.rs       - Module defined in own file
❌ src/bundle/mod.rs       - Don't create this
```

**Rationale**: Project convention for cleaner file structure

**See**: [overview.md line 111](overview.md)

### 4.2 Test Organization

**Rule**: Follow the established test hierarchy.

```
rust/bundlebase/src/
├── bundle.rs                 - Implementation
├── tests/
│   └── bundle_tests.rs       - Rust integration tests

python/tests/
└── test_e2e.py              - Python E2E tests
```

**Test Philosophy**:
- **Rust unit tests**: Test individual Rust functions/modules
- **Rust integration tests**: Test Rust API integration
- **Python E2E tests**: Test Python binding works end-to-end
- **Python tests do NOT test Rust logic** - that's what Rust tests are for

**Anti-pattern**:
```python
# ❌ WRONG: Testing Rust logic in Python
def test_filter_validates_sql_syntax():
    # This tests Rust's SQL parser, not Python binding
    with pytest.raises(Exception):
        await container.filter("INVALID SQL")
```

**Correct**:
```python
# ✅ CORRECT: Testing Python binding works
async def test_filter_returns_filtered_results():
    # Tests that Python binding correctly calls Rust and returns results
    c = await bundlebase.create().attach("data.parquet")
    c = c.filter("age >= 18")
    df = await c.to_pandas()
    assert len(df) > 0
```

**See**: [testing.md](testing.md)

## 5. Performance Constraints

### 5.1 Streaming Only (CRITICAL)

**Already covered comprehensively in section 1.2**

Key points:
- ✅ Use `execute_stream()`, never `collect()`
- ✅ Return streaming objects to Python
- ✅ Process batches incrementally in Python
- ❌ Never accumulate full dataset in memory

### 5.2 Memory Efficiency

**Rule**: Design for constant memory usage, not proportional to dataset size.

#### Memory-Efficient Patterns

```rust
// ✅ CORRECT: Streaming aggregation
async fn count_rows(stream: SendableRecordBatchStream) -> Result<usize> {
    let mut total = 0;
    let mut stream = stream;

    while let Some(batch) = stream.next().await {
        let batch = batch?;
        total += batch.num_rows();  // Batch freed after this iteration
    }

    Ok(total)
}

// ❌ WRONG: Accumulates all batches
async fn count_rows(df: DataFrame) -> Result<usize> {
    let batches = df.collect().await?;  // All in memory
    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    Ok(total)
}
```

**Design Principle**: If the operation can be computed incrementally on batches, it should be.

**See**: [development.md](development.md), [anti-patterns.md](anti-patterns.md)

## 6. Code Quality Constraints

### 6.1 No Over-Engineering

**Rule**: Only add features that are directly requested or clearly necessary.

**Don't add**:
- Error handling for scenarios that can't happen (trust internal code)
- Validation beyond system boundaries (user input, external APIs)
- Premature abstractions for one-time operations
- Feature flags or backward-compatibility shims (project not launched)

**Example**:
```rust
// ❌ WRONG: Over-engineered
pub fn process_internal_data(data: Vec<Record>) -> Result<ProcessedData> {
    // Validating internal data we control
    if data.is_empty() {
        return Err("empty data".into());
    }

    // Handling impossible case
    if data[0].id.is_none() {
        return Err("missing id".into());
    }

    // Process...
}

// ✅ CORRECT: Trust internal guarantees
pub fn process_internal_data(data: Vec<Record>) -> ProcessedData {
    // Internal function, data is guaranteed valid
    // Process directly without unnecessary checks
}
```

**Rationale**: Premature complexity harms maintainability. Add complexity only when needed.

**See**: [development.md lines 92-98](development.md)

### 6.2 No Backward Compatibility Constraints

**Rule**: Project is not launched. Break freely when improvements warrant it.

**Prohibited patterns**:
- Keeping deprecated methods "for compatibility"
- Feature flags for new behavior
- Renaming unused variables with `_` prefix
- Re-exporting removed types
- Adding `// removed` comments for deleted code

**Example**:
```rust
// ❌ WRONG: Maintaining compatibility
#[deprecated(since = "0.2.0", note = "Use new_method instead")]
pub fn old_method(&self) -> Result<Data> {
    self.new_method()  // Wrapper for compatibility
}

// ✅ CORRECT: Just delete it
// old_method is gone, users update their code
```

**Rationale**: Pre-launch flexibility enables better design decisions

**See**: [development.md line 93](development.md)

## 7. Documentation Constraints

### 7.1 Read Context Before Coding

**Rule**: Before generating code, read relevant `.context/` documentation files.

**Required reading**:
- This file (`ai-rules.md`) - Hard constraints
- `anti-patterns.md` - What NOT to do
- Relevant domain files (architecture, API, testing, etc.)

**Rationale**: Prevents generating code that violates established patterns

**See**: [README.md](README.md)

### 7.2 Update Documentation with Code Changes

**Rule**: When making architectural changes, update relevant `.context/` files in the same PR.

**Files to update**:
- Architecture changes → `architecture.md`
- New API patterns → `python-api.md` or `python-bindings.md`
- Performance changes → This file + `anti-patterns.md`
- New workflows → `workflows.md`

**Rationale**: Keeps documentation in sync with code

## 8. Technology-Specific Constraints

### 8.1 DataFusion Patterns

**Rule**: Use DataFusion's streaming execution model.

```rust
// ✅ CORRECT: Streaming execution plan
let df = ctx.sql("SELECT * FROM table").await?;
let stream = df.execute_stream().await?;

// ❌ WRONG: Collecting results
let df = ctx.sql("SELECT * FROM table").await?;
let batches = df.collect().await?;
```

### 8.2 Arrow Memory Management

**Rule**: Trust Arrow's reference counting. Don't clone unless necessary.

```rust
// ✅ CORRECT: Shared reference
let schema: SchemaRef = batch.schema();  // Arc<Schema>, cheap clone

// ❌ WRONG: Deep copy
let schema = (*batch.schema()).clone();  // Expensive, unnecessary
```

### 8.3 Tokio Runtime

**Rule**: Use existing runtime, don't create new ones.

```rust
// ✅ CORRECT: Use existing runtime
let result = tokio::spawn(async_task).await?;

// ❌ WRONG: Creating new runtime
let rt = Runtime::new()?;  // Don't do this
let result = rt.block_on(async_task)?;
```

## Summary Checklist

Before generating any code, verify:

- [ ] Understand the feature requirement
- [ ] Read relevant `.context/` documentation
- [ ] Rust implementation comes before Python bindings
- [ ] No `collect()` - always streaming execution
- [ ] No `.unwrap()` - proper error handling
- [ ] PyO3 code only in `python/` module
- [ ] Mutable operations return `&mut Self`
- [ ] Operations are lazy, executed only during query
- [ ] Python methods registered in `__init__.py`
- [ ] Tests follow established hierarchy
- [ ] No over-engineering or premature optimization
- [ ] No backward compatibility constraints
- [ ] Documentation updated if architectural changes made

**When in doubt**: Ask for clarification rather than guessing. Prefer simplicity over flexibility.
