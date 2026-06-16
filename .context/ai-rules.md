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

### 2.1 Three-Tier Mutability Boundaries

**Rule**: Respect the three-tier architecture. User-facing mutation goes through `BundleBuilder` only.

#### The Three Types

| Type | Role | Mutation |
|------|------|----------|
| **BundleFacade trait** | Common read/query interface | None — read/query methods only |
| **BundleBuilder** | Working copy with change tracking | `&self -> Result<&Self>` methods using interior mutability, wrapped in `run_command`/`do_change` |
| **Bundle** | Committed/persistent state | Interior mutability via `Arc<RwLock<...>>` fields, but ONLY through `pub(crate) apply_operation()` — never expose public mutation on `Bundle` |

#### Code Patterns

```rust
// ✅ CORRECT: Modification methods on BundleBuilder (or BundleBuilderExt)
// take &self and return Result<&Self>, routed through change tracking
async fn filter(&self, query: &str, params: Vec<ScalarValue>)
    -> Result<&Self, BundlebaseError>;

// ✅ CORRECT: Bundle mutation only via the operation pipeline
impl Bundle {
    pub(crate) async fn apply_operation(&self, op: AnyOperation)
        -> Result<(), BundlebaseError> { /* check, apply, record, clear cache */ }
}

// ❌ WRONG: Public ad-hoc mutation on Bundle that bypasses the operation log
impl Bundle {
    pub fn set_packs(&self, packs: HashMap<ObjectId, Arc<Pack>>) {
        *self.packs.write() = packs; // bypasses check/version/operation recording
    }
}
```

**Rationale**: All state changes must flow through recorded operations so they version, persist, and replay correctly

**See**: [architecture.md](architecture.md), [decisions/004](decisions/004-three-tier-architecture.md)

### 2.2 Modification Methods: `&self -> Result<&Self>`

**Rule**: Modification methods take `&self`, mutate via interior mutability, and return `Result<&Self, BundlebaseError>` for fluent chaining with `?`. The types are shared via `Arc`, so `&mut self` signatures are not possible.

```rust
// ✅ CORRECT: Chainable API (actual signatures from builder.rs / builder_ext.rs)
pub async fn commit(&self, message: &str) -> Result<&Self, BundlebaseError>;
async fn attach(&self, path: &str, pack: Option<&str>) -> Result<&Self, BundlebaseError>;
async fn rename_column(&self, old_name: &str, new_name: &str) -> Result<&Self, BundlebaseError>;

// Usage: Fluent chaining
builder.attach("data.parquet", None).await?
    .filter("SELECT * FROM bundle WHERE age >= 18", vec![]).await?
    .commit("load and filter").await?;

// ❌ WRONG: &mut self — cannot be called through Arc<BundleBuilder>
pub fn filter(&mut self, expr: &str) -> &mut Self { ... }

// ❌ WRONG: Returns () — breaks chaining (unless the method's purpose is to
// return a value, e.g. delete() returns the affected row count)
pub async fn filter(&self, expr: &str) -> Result<(), BundlebaseError> { ... }
```

**Rationale**: Enables fluent chaining while supporting `Arc`-shared access from async tasks, server sessions, and Python bindings

**See**: [decisions/005](decisions/005-mutable-operations.md), [architecture.md](architecture.md)

### 2.3 Internal Column Naming (col_<id>)

**Rule**: All internal column references use the stable `col_<column_id>` format. User-visible names are only applied at the final output boundary.

**How it works**:
1. `DataBlock.schema()` returns fields named `col_<hex_id>` (not physical file names)
2. Operations' `apply_dataframe()` methods work with `col_<id>` columns throughout
3. The command layer translates user input to `col_<id>` before creating operations
4. `dataframe()` in `bundle.rs` applies a final rename from `col_<id>` → user-visible names

**When adding a new command that stores SQL**: Always translate user-provided SQL before storing it in an operation — use `builder.translate_sql(sql)` (or `BundleSchema::translate_sql()` inside an operation) to rewrite user-visible names to `col_<id>` references.

**When adding a new operation with `apply_dataframe()`**: Columns in the incoming DataFrame are `col_<id>`. Use `bundle_schema.internal_name(&id)` / `bundle_schema::generate_internal_name(&id)` to reference columns. The `bundle_schema: &mut BundleSchema` parameter tracks `ColumnId → user_name` for the final rename step (e.g. `RenameColumnOp` just calls `bundle_schema.insert(id, new_name)`).

**Key files**: `bundle/bundle_schema.rs` (helpers: `generate_internal_name()`, `parse_internal_name()`, `translate_sql()`, `rename_to_real_names()`), `bundle/data_block.rs`, `bundle/schema_rename_filter.rs`

### 2.4 Lazy Evaluation

**Rule**: Operations are recorded when called, executed only during query time.

#### How It Works

```python
# These operations are RECORDED, not executed
c = c.filter("age >= 18")    # Adds FilterBlock to operations list
c = c.query("SELECT name FROM bundle")  # Adds query to operations list

# THIS executes all recorded operations
df = await c.to_pandas()     # NOW queries run
```

#### Implementation Pattern

```rust
// ✅ CORRECT: Record the operation; Bundle::apply_operation validates (check),
// applies side effects (apply), records the op, and clears the cached DataFrame
builder.apply_operation(FilterOp::new(query, params).into()).await?;

// Execution happens lazily in Bundle::dataframe() (bundle.rs):
// rebuilds from the base pack table by running each recorded op's
// apply_dataframe() in order, then renames col_<id> → user-visible names
let df = bundle.dataframe().await?;

// ❌ WRONG: Materializing/executing data inside the operation-recording path.
// Operations must not run the query when recorded — data transformation
// belongs in the op's apply_dataframe(), which runs at rebuild time.
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

### 3.2 Python Method Registration

**Rule**: New builder methods must be registered in `python/src/bundlebase/__init__.py` to support fluent chaining and the sync API.

#### Registration Pattern

```python
# python/src/bundlebase/__init__.py

# 1. Add the original PyO3 method to the _original_methods dict
_original_methods = {
    ...
    "my_operation": _PyBundleBuilder.my_operation,
}
# (each entry is passed to register_original_method for the chain machinery)

# 2. If it mutates the bundle and returns the builder, add it to
#    mutation_methods so it gets wrapped to return an OperationChain
mutation_methods = [
    "attach", "drop_column", "rename_column", "filter", ...,
    "my_operation",
]
```

Mutation methods are wrapped by `_wrap_mutation_method()` to return an `OperationChain` (see `chain.py`), which queues calls and executes them on `await`. The sync API (`bundlebase.sync`) builds on the same registration.

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
- [ ] PyO3 code only in `rust/bundlebase-python/`
- [ ] Modification methods take `&self` and return `Result<&Self>`
- [ ] Operations are lazy: data transformation only in `apply_dataframe()`
- [ ] Python methods registered in `__init__.py`
- [ ] Tests follow established hierarchy
- [ ] No over-engineering or premature optimization
- [ ] No backward compatibility constraints
- [ ] Documentation updated if architectural changes made

**When in doubt**: Ask for clarification rather than guessing. Prefer simplicity over flexibility.
