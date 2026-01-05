# Anti-Patterns - What NOT to Do

This document catalogs discouraged approaches in the Bundlebase project with concrete examples. If you find yourself writing code that matches these patterns, stop and reconsider.

## 1. Performance Anti-Patterns

### ❌ NEVER: Use collect() to Materialize Datasets

**The Problem**: Calling `.collect()` on a DataFrame materializes the entire dataset in memory, causing extreme memory usage.

**Why It's Bad**:
| File Size | collect() Memory | execute_stream() Memory |
|-----------|------------------|------------------------|
| 10GB | ~30GB peak RAM (3x) | ~50MB peak RAM |
| 100GB | ~300GB peak RAM | ~50MB peak RAM |
| 1TB | Out of Memory | ~50MB peak RAM |

**Wrong**:
```rust
// ❌ BAD: Materializes entire dataset
async fn process_data(df: DataFrame) -> Result<Vec<RecordBatch>> {
    let batches = df.collect().await?;  // ENTIRE dataset in memory
    Ok(batches)
}

// ❌ BAD: Even collecting to count is wrong
async fn count_rows(df: DataFrame) -> Result<usize> {
    let batches = df.collect().await?;  // Loads everything just to count
    Ok(batches.iter().map(|b| b.num_rows()).sum())
}
```

**Correct**:
```rust
// ✅ GOOD: Streaming execution
async fn process_data(df: DataFrame) -> Result<SendableRecordBatchStream> {
    let stream = df.execute_stream().await?;  // Constant memory
    Ok(stream)
}

// ✅ GOOD: Stream to count
async fn count_rows(df: DataFrame) -> Result<usize> {
    let mut stream = df.execute_stream().await?;
    let mut total = 0;
    while let Some(batch) = stream.next().await {
        total += batch?.num_rows();  // Process one batch at a time
    }
    Ok(total)
}
```

**See Also**: [ai-rules.md section 1.2](ai-rules.md), [development.md lines 102-169](development.md)

### ❌ NEVER: Accumulate All Batches in Vec

**The Problem**: Collecting batches into a vector defeats streaming and causes memory bloat.

**Wrong**:
```python
# ❌ BAD: Accumulates all batches in memory
async def to_custom_format(container):
    all_batches = []
    async for batch in stream_batches(container):
        all_batches.append(batch)  # Holds ALL batches in memory

    # Now you have the entire dataset in RAM
    return combine_batches(all_batches)

# ❌ BAD: Getting Arrow table defeats streaming
async def process_large_file(container):
    arrow_table = await _get_arrow_table(container)  # Full load
    return heavy_computation(arrow_table)  # Too late, already OOM
```

**Correct**:
```python
# ✅ GOOD: Process and release incrementally
async def to_custom_format(container):
    results = []
    async for batch in stream_batches(container):
        chunk = process_batch(batch)  # Process one batch
        results.append(chunk)  # Only append processed result
        # batch is freed here
    return combine_results(results)

# ✅ GOOD: Incremental computation
async def process_large_file(container):
    accumulated_result = None
    async for batch in stream_batches(container):
        chunk_result = compute_on_batch(batch)
        accumulated_result = merge(accumulated_result, chunk_result)
    return accumulated_result
```

**See Also**: [ai-rules.md section 1.2](ai-rules.md)

### ❌ NEVER: Call as_pyarrow() for Large Datasets

**The Problem**: `as_pyarrow()` creates a full PyArrow table in memory.

**Wrong**:
```python
# ❌ BAD: Materializes for large datasets
async def export_to_custom_format(container):
    arrow_table = await container.as_pyarrow()  # Full materialization
    return custom_format.from_arrow(arrow_table)
```

**Correct**:
```python
# ✅ GOOD: Use stream_batches() instead
async def export_to_custom_format(container):
    async for batch in stream_batches(container):
        chunk = custom_format.from_arrow_batch(batch)
        yield chunk  # Stream output too

# ✅ GOOD: Use built-in streaming converters
async def to_pandas_correctly(container):
    # to_pandas() streams internally - safe for large data
    df = await container.to_pandas()
    return df
```

**When it's OK**: Small datasets (<1GB) where convenience matters more than memory efficiency.

**See Also**: [python-api.md](python-api.md)

### ❌ NEVER: Return Vec<RecordBatch> to Python

**The Problem**: Returning collected batches to Python defeats the entire streaming architecture.

**Wrong**:
```rust
// ❌ BAD: Collects before returning to Python
#[pymethods]
impl PyBundle {
    fn get_data<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyAny>>> {
        let batches = self.inner.collect().await?;  // Full load
        // Convert and return
    }
}
```

**Correct**:
```rust
// ✅ GOOD: Return streaming object
#[pymethods]
impl PyBundle {
    fn as_pyarrow_stream<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let stream = self.inner.execute_stream().await?;  // Stream
        Ok(PyRecordBatchStream::new(stream, schema).into_py(py))
    }
}
```

**See Also**: [ai-rules.md section 1.2](ai-rules.md), [python-bindings.md](python-bindings.md)

## 2. Rust Code Anti-Patterns

### ❌ NEVER: Use .unwrap()

**The Problem**: The codebase enforces `#![deny(clippy::unwrap_used)]` - `.unwrap()` calls will not compile.

**Wrong**:
```rust
// ❌ COMPILE ERROR: clippy::unwrap_used is denied
let value = option.unwrap();
let result = operation().unwrap();
let converted = string.parse::<i64>().unwrap();
```

**Correct**:
```rust
// ✅ GOOD: Use ? operator
let value = option.ok_or_else(|| BundlebaseError::from("missing value"))?;
let result = operation()?;

// ✅ GOOD: Use pattern matching
match option {
    Some(val) => process(val),
    None => return Err("value not found".into()),
}

// ✅ GOOD: Use .map_err() for conversion
let result = rust_operation()
    .map_err(|e| PyErr::new::<PyRuntimeError, _>(format!("Error: {}", e)))?;
```

**Why**: Forces explicit error handling, prevents panics in production.

**See Also**: [ai-rules.md section 1.3](ai-rules.md), `rust/bundlebase/src/lib.rs:1`

### ❌ NEVER: Put PyO3 Code Outside python/ Module

**The Problem**: Mixing Python bindings with core Rust logic creates tight coupling.

**Wrong**:
```rust
// ❌ BAD: In rust/bundlebase/src/bundle.rs
use pyo3::prelude::*;

impl Bundlebase {
    #[pymethod]  // PyO3 in core module!
    pub fn query(&self) -> PyResult<DataFrame> {
        // ...
    }
}
```

**Correct**:
```rust
// ✅ GOOD: Core logic in rust/bundlebase/src/bundle.rs (NO PyO3)
impl Bundlebase {
    pub fn query(&self) -> Result<DataFrame> {
        // Pure Rust implementation
    }
}

// ✅ GOOD: Python wrapper in rust/bundlebase-python/src/bundle.rs
use pyo3::prelude::*;

#[pyclass]
struct PyBundle {
    inner: Bundlebase,
}

#[pymethods]
impl PyBundle {
    fn query(&self) -> PyResult<PyDataFrame> {
        self.inner.query()
            .map_err(|e| PyErr::new::<PyRuntimeError, _>(e.to_string()))?;
        // Convert and return
    }
}
```

**Why**: Enables other language bindings, keeps core logic independent.

**See Also**: [ai-rules.md section 3.1](ai-rules.md)

### ❌ NEVER: Create mod.rs Files

**The Problem**: Project convention prefers named module files.

**Wrong**:
```
src/
├── bundle/
│   ├── mod.rs       # ❌ Don't create this
│   ├── builder.rs
│   └── state.rs
```

**Correct**:
```
src/
├── bundle.rs        # ✅ Module defined in own file
├── operations.rs    # ✅ All public modules as named files
└── versioning.rs
```

**Why**: Cleaner file structure, easier navigation.

**See Also**: [ai-rules.md section 4.1](ai-rules.md), [overview.md](overview.md)

### ❌ NEVER: Return &mut Self from Read-Only Methods

**The Problem**: Violates immutability contract of read-only containers.

**Wrong**:
```rust
// ❌ BAD: Read-only Bundle shouldn't return &mut self
impl Bundle {
    pub fn get_schema(&mut self) -> &mut Self {
        // This is a read operation, why return &mut self?
        self
    }
}
```

**Correct**:
```rust
// ✅ GOOD: Read operations use &self
impl Bundle {
    pub fn schema(&self) -> &Schema {
        &self.state.schema  // Immutable reference
    }
}

// ✅ GOOD: Mutable operations on BundleBuilder use &mut self
impl BundleBuilder {
    pub fn filter(&mut self, expr: &str) -> &mut Self {
        self.operations.push(Operation::Filter(expr.to_string()));
        self  // Chainable mutations
    }
}
```

**Why**: Maintains clear separation between read-only and mutable operations.

**See Also**: [ai-rules.md section 2.1](ai-rules.md), [architecture.md](architecture.md)

## 3. Python Binding Anti-Patterns

### ❌ NEVER: Test Rust Logic in Python Tests

**The Problem**: Python tests should verify bindings work, not test Rust business logic.

**Wrong**:
```python
# ❌ BAD: Testing Rust SQL parser from Python
def test_filter_validates_sql_syntax():
    """This tests Rust logic, not Python binding"""
    c = await bundlebase.create()

    # Testing parser edge cases
    with pytest.raises(Exception):
        c.filter("SELECT INVALID SQL HERE")

    with pytest.raises(Exception):
        c.filter("age = = 18")  # Double equals
```

**Correct**:
```python
# ✅ GOOD: Testing Python binding works E2E
async def test_filter_returns_filtered_data():
    """Tests that Python can call Rust filter and get results"""
    c = await bundlebase.create()
    await c.attach("test_data.parquet")
    c = c.filter("age >= 18")

    df = await c.to_pandas()
    assert all(df['age'] >= 18)  # Verify filtering worked

# ✅ GOOD: Testing async/await works
async def test_async_methods_work():
    """Tests that async bridge works correctly"""
    c = await bundlebase.create()
    assert c is not None  # Binding succeeded
```

**Rust tests are for Rust logic**:
```rust
// ✅ GOOD: Test parser in Rust tests
#[test]
fn test_parse_invalid_sql() {
    let result = parse_filter("INVALID SQL");
    assert!(result.is_err());
}
```

**Why**: Tests belong at the layer they're testing. Python tests for Python API, Rust tests for Rust logic.

**See Also**: [ai-rules.md section 4.2](ai-rules.md), [testing.md](testing.md)

### ❌ NEVER: Add Method Without Registering in __init__.py

**The Problem**: Methods must be registered to work with sync API and fluent chaining.

**Wrong**:
```rust
// Added to rust/bundlebase-python/src/bundle.rs
#[pymethods]
impl PyBundleBuilder {
    fn new_operation(&mut self, param: String) -> PyResult<()> {
        // Implementation
    }
}

// ❌ FORGOT to register in python/src/bundlebase/__init__.py
// Result: Method doesn't work in bundlebase.sync module
```

**Correct**:
```python
# ✅ GOOD: Register in python/src/bundlebase/__init__.py
for cls in [Bundle, BundleBuilder]:
    # Register async method
    register_async_method(cls, 'new_operation', 'new_operation')

    # Register sync wrapper
    register_sync_method(cls, 'new_operation', 'new_operation')
```

**Why**: Registration enables both async and sync APIs, ensures fluent chaining works.

**See Also**: [ai-rules.md section 3.2](ai-rules.md), [sync-api.md](sync-api.md)

### ❌ NEVER: Forget to Make Methods Async

**The Problem**: Inconsistent API - some methods async, others not.

**Wrong**:
```rust
// ❌ BAD: Sync method in async API
#[pymethods]
impl PyBundle {
    fn to_pandas(&self, py: Python) -> PyResult<PyDataFrame> {
        // This should be async for consistency
    }
}
```

**Correct**:
```rust
// ✅ GOOD: Async method for consistency
#[pymethods]
impl PyBundle {
    fn to_pandas<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = self.inner.to_pandas().await?;
            Ok(result)
        })
    }
}
```

**Why**: Consistent async API across all methods. Sync variant provided automatically via `bundlebase.sync`.

**See Also**: [ai-rules.md section 3.2](ai-rules.md), [python-bindings.md](python-bindings.md)

## 4. Architecture Anti-Patterns

### ❌ NEVER: Mutate Bundle (Read-Only)

**The Problem**: Bundle is designed to be immutable - attempting mutation breaks the architecture.

**Wrong**:
```rust
// ❌ BAD: Trying to mutate immutable Bundle
impl Bundle {
    pub fn add_operation(&mut self, op: Operation) {
        // Type system should prevent this
        self.operations.push(op);
    }
}
```

**Correct**:
```rust
// ✅ GOOD: Only BundleBuilder has mutation methods
impl BundleBuilder {
    pub fn add_operation(&mut self, op: Operation) -> &mut Self {
        self.operations.push(op);
        self
    }
}

// ✅ GOOD: Bundle only has read operations
impl Bundle {
    pub fn operations(&self) -> &[Operation] {
        &self.operations  // Read-only access
    }
}
```

**Why**: Three-tier architecture depends on clear mutability boundaries.

**See Also**: [ai-rules.md section 2.1](ai-rules.md), [architecture.md](architecture.md)

### ❌ NEVER: Skip Operations During Query

**The Problem**: Lazy evaluation requires ALL recorded operations to execute during query.

**Wrong**:
```rust
// ❌ BAD: Skipping operations
async fn execute(&self) -> Result<DataFrame> {
    let mut df = self.initial_dataframe.clone();

    for op in &self.operations {
        // Skipping some operations based on flags
        if op.should_apply() {  // ❌ Conditional execution
            df = op.apply(df).await?;
        }
    }

    Ok(df)
}
```

**Correct**:
```rust
// ✅ GOOD: Apply ALL operations
async fn execute(&self) -> Result<DataFrame> {
    let mut df = self.initial_dataframe.clone();

    // Apply every recorded operation
    for op in &self.operations {
        df = op.apply(df).await?;
    }

    Ok(df)
}
```

**Why**: Operations are the contract - what's recorded must execute.

**See Also**: [ai-rules.md section 2.3](ai-rules.md), [architecture.md](architecture.md)

### ❌ NEVER: Maintain Compatibility for Unlaunched Project

**The Problem**: Project isn't launched - backwards compatibility is unnecessary burden.

**Wrong**:
```rust
// ❌ BAD: Deprecated methods for compatibility
#[deprecated(since = "0.2.0", note = "Use new_method instead")]
pub fn old_method(&self) -> Result<Data> {
    self.new_method()  // Wrapper for compatibility
}

// ❌ BAD: Feature flags for behavior changes
pub fn process(&self, use_new_algorithm: bool) -> Result<Data> {
    if use_new_algorithm {
        self.new_algorithm()
    } else {
        self.old_algorithm()  // Why keep this?
    }
}

// ❌ BAD: Keeping unused parameters
pub fn filter(&mut self, expr: &str, _legacy_mode: bool) -> &mut Self {
    // _legacy_mode ignored but kept for "compatibility"
}
```

**Correct**:
```rust
// ✅ GOOD: Just delete old methods
// old_method is GONE - users update their code

// ✅ GOOD: Use new algorithm directly
pub fn process(&self) -> Result<Data> {
    self.new_algorithm()  // One way to do it
}

// ✅ GOOD: Remove unused parameters
pub fn filter(&mut self, expr: &str) -> &mut Self {
    // Clean API
}
```

**Why**: Pre-launch flexibility enables better design. Break freely when improvements warrant it.

**See Also**: [ai-rules.md section 6.2](ai-rules.md), [development.md](development.md)

## 5. Development Anti-Patterns

### ❌ NEVER: Over-Engineer Edge Cases

**The Problem**: Adding validation/error handling for impossible scenarios adds complexity without value.

**Wrong**:
```rust
// ❌ BAD: Validating internal data we control
fn process_internal_data(data: Vec<Record>) -> Result<ProcessedData> {
    // Over-validation of internal data
    if data.is_empty() {
        return Err("empty data".into());  // Can this even happen?
    }

    // Checking impossible conditions
    for record in &data {
        if record.id.is_none() {
            return Err("missing id".into());  // Our code creates these
        }
        if record.timestamp < 0 {
            return Err("invalid timestamp".into());  // Impossible
        }
    }

    // Finally do actual work...
}
```

**Correct**:
```rust
// ✅ GOOD: Trust internal guarantees
fn process_internal_data(data: Vec<Record>) -> ProcessedData {
    // Internal function - data is guaranteed valid by our code
    // Process directly without unnecessary checks
    data.into_iter().map(|r| transform(r)).collect()
}

// ✅ GOOD: Validate only at system boundaries
pub fn attach(&mut self, path: &str) -> Result<&mut Self> {
    // User input - MUST validate
    if path.is_empty() {
        return Err("path cannot be empty".into());
    }

    // Validate path exists
    if !Path::new(path).exists() {
        return Err(format!("path not found: {}", path).into());
    }

    // Proceed with validated input
    self.internal_attach(path);  // Trusts input is valid
    Ok(self)
}
```

**Rule**: Validate at system boundaries (user input, external APIs). Trust internal code.

**See Also**: [ai-rules.md section 6.1](ai-rules.md), [development.md](development.md)

### ❌ NEVER: Add Premature Optimization

**The Problem**: Optimizing before measuring creates complexity without proven benefit.

**Wrong**:
```rust
// ❌ BAD: Complex caching for unproven bottleneck
struct Container {
    data: Vec<Record>,
    schema_cache: Arc<RwLock<HashMap<String, Schema>>>,  // Premature?
    query_cache: Arc<RwLock<LruCache<String, Result>>>,  // Needed?
    metadata_cache: OnceCell<Metadata>,                   // Really?
}

impl Container {
    fn get_schema(&self) -> Schema {
        // Complex caching logic before proving it's slow
        let cache_key = self.compute_cache_key();
        if let Some(cached) = self.schema_cache.read().get(&cache_key) {
            return cached.clone();
        }
        // ... 30 more lines of cache management
    }
}
```

**Correct**:
```rust
// ✅ GOOD: Simple, direct implementation
struct Container {
    data: Vec<Record>,
    schema: Schema,  // Just store it
}

impl Container {
    fn schema(&self) -> &Schema {
        &self.schema  // Direct access, simple
    }
}

// ✅ IF profiling shows schema() is a bottleneck, THEN optimize
```

**Rule**: Profile first, optimize second. Complexity needs justification.

**See Also**: [ai-rules.md section 6.1](ai-rules.md)

### ❌ NEVER: Generate Code Without Reading Context

**The Problem**: Generating code that violates documented patterns because context wasn't consulted.

**Wrong Workflow**:
1. User asks: "Add method to export to Excel"
2. AI immediately generates code without reading docs
3. Code uses `collect()` (violates streaming)
4. Code uses `.unwrap()` (doesn't compile)
5. Code doesn't register method in `__init__.py` (breaks sync API)

**Correct Workflow**:
1. User asks: "Add method to export to Excel"
2. AI reads [ai-rules.md](ai-rules.md) - sees streaming requirement, no unwrap
3. AI reads [python-bindings.md](python-bindings.md) - sees registration requirement
4. AI generates code that:
   - Uses `execute_stream()` for memory efficiency
   - Uses `?` operator for error handling
   - Registers method in `__init__.py`
   - Follows established patterns

**Rule**: ALWAYS read relevant `.context/` files before generating code.

**See Also**: [ai-rules.md section 7.1](ai-rules.md), [README.md](README.md)

## Summary: Top 5 Anti-Patterns to Avoid

1. **Using `collect()`** - Always stream with `execute_stream()`
2. **Using `.unwrap()`** - Always use `?` operator or pattern matching
3. **Testing Rust logic in Python** - Test bindings in Python, logic in Rust
4. **PyO3 code outside python/ module** - Keep bindings separate from core
5. **Over-engineering** - Add complexity only when needed

**When in doubt**: Consult [ai-rules.md](ai-rules.md) for hard constraints, this file for what to avoid, and [README.md](README.md) for navigation.
