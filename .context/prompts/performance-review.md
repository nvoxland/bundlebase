# Template: Performance Review

Use this template when optimizing performance, investigating slow operations, or reviewing code for efficiency. Performance issues in bundlebase typically relate to memory usage or query execution speed.

## When to Use This Template

- Queries are slower than expected
- Memory usage is growing beyond reasonable limits
- CPU usage is unexpectedly high
- Need to validate streaming execution
- Preparing for large dataset testing
- Reviewing new feature for performance

## Required Reading

Before starting, read:

1. **[decisions/003-streaming-only.md](../decisions/003-streaming-only.md)** - Why streaming is critical
2. **[architecture.md](../architecture.md#streaming-execution-architecture)** - How streaming works
3. **[python-api.md](../python-api.md#streaming-api-for-large-datasets)** - Python streaming patterns
4. **CLAUDE.md Performance Guidelines** section

## Critical Performance Rules

All code MUST follow:

- ✅ **Streaming execution only** - Use `execute_stream()`, never `collect()`
- ✅ **Constant memory usage** - Memory should not grow with dataset size
- ✅ **Lazy evaluation** - Don't execute until query time
- ✅ **Predicate pushdown** - Push filters to data source when possible
- ✅ **Incremental processing** - Process data in batches
- ✅ **Avoid unnecessary clones** - Minimize data copies

## Performance Investigation Checklist

### 1. Identify Performance Issue

- [ ] Measure baseline performance (time, memory, CPU)
- [ ] Identify slow operation or function
- [ ] Determine if issue is CPU-bound, memory-bound, or I/O-bound
- [ ] Check if issue occurs with small data (logic) or only large data (streaming)
- [ ] Get reproducible test case

**Measurement tools:**
```bash
# Time execution
time poetry run python script.py

# Memory profiling (Python)
poetry add --dev memory_profiler
python -m memory_profiler script.py

# Rust profiling
cargo build --release
cargo flamegraph --bin bundlebase
```

### 2. Check for Streaming Violations

**This is the #1 cause of performance issues in bundlebase.**

- [ ] Search codebase for `.collect()` calls
- [ ] Verify `execute_stream()` is used for all queries
- [ ] Check Python methods use streaming internally
- [ ] Ensure batches are processed incrementally, not accumulated

**Search for violations:**
```bash
# Find collect() calls in Rust
rg "\.collect\(\)" --type rust

# Find potential streaming violations
rg "Vec<RecordBatch>" --type rust  # Accumulating batches?
```

**Example violation:**
```rust
// ❌ WRONG: Loads entire dataset into memory
async fn to_pandas(&self) -> Result<Vec<RecordBatch>> {
    let df = self.to_dataframe().await?;
    let batches = df.collect().await?;  // 🚨 PERFORMANCE BUG
    Ok(batches)
}

// ✅ CORRECT: Streaming execution
async fn to_pandas(&self) -> Result<PyObject> {
    let df = self.to_dataframe().await?;
    let mut stream = df.execute_stream().await?;  // ✅ Streaming

    // Process incrementally
    while let Some(batch) = stream.next().await {
        let batch = batch?;
        // Send to Python incrementally
    }
}
```

### 3. Profile Memory Usage

- [ ] Test with dataset larger than RAM
- [ ] Monitor memory usage during execution
- [ ] Verify memory stays constant (not growing with data size)
- [ ] Check for memory leaks (use valgrind or cargo-leak)

**Expected behavior:**
- Small file (100MB): ~50MB RAM usage
- Large file (10GB): ~50MB RAM usage (same!)
- Memory usage independent of dataset size = streaming works

**If memory grows with dataset size → streaming violation!**

### 4. Analyze Query Plan

- [ ] Check if DataFusion can optimize the query
- [ ] Verify predicate pushdown is happening
- [ ] Look for unnecessary operations
- [ ] Check if operations can be combined

**Example:**
```rust
// Check DataFusion query plan
let logical_plan = df.logical_plan();
println!("Logical plan: {:?}", logical_plan);

// Look for:
// ✅ Predicate pushdown (filter pushed to ParquetExec)
// ✅ Projection pushdown (only needed columns read)
// ❌ Full table scan when filter available
```

### 5. Identify Bottlenecks

Common bottlenecks in bundlebase:

- [ ] **I/O bound** - Reading from disk/network
  - Solution: Use columnar format (Parquet), enable compression
- [ ] **CPU bound** - Complex filters or computations
  - Solution: Simplify expressions, use DataFusion built-ins
- [ ] **Memory bound** - Not streaming, accumulating data
  - Solution: Replace `collect()` with `execute_stream()`
- [ ] **Python overhead** - Crossing Rust/Python boundary
  - Solution: Process larger batches, minimize conversions

### 6. Implement Optimization

Choose optimization strategy:

**A. Fix Streaming Violation**
```rust
// Before: collect() loads all data
let batches = df.collect().await?;

// After: execute_stream() processes incrementally
let mut stream = df.execute_stream().await?;
while let Some(batch) = stream.next().await {
    process_batch(batch?);
}
```

**B. Reduce Allocations**
```rust
// Before: Unnecessary clone
let data = original.clone();  // Expensive for large data

// After: Use reference or Arc
let data = Arc::clone(&original);  // Cheap pointer copy
```

**C. Push Operations Down**
```rust
// Before: Filter after reading all data
let df = ctx.read_parquet("file.parquet").await?;
let filtered = df.filter(col("age").gt(lit(18)))?;

// After: Same code, but DataFusion pushes filter to ParquetExec automatically
// (This is why we use DataFusion!)
```

**D. Batch Size Tuning**
```rust
// Adjust batch size for Python transfer
let options = ParquetReadOptions {
    batch_size: 65536,  // Tune based on profiling
    ..Default::default()
};
```

### 7. Measure Improvement

- [ ] Re-run performance test
- [ ] Compare before/after metrics (time, memory, CPU)
- [ ] Verify memory usage is constant
- [ ] Test with multiple dataset sizes
- [ ] Document improvement (e.g., "50% faster", "90% less memory")

**Example:**
```
Before: 10GB file uses 30GB RAM, 120s
After:  10GB file uses 50MB RAM, 45s
Result: 600x less memory, 2.6x faster
```

### 8. Add Performance Test

- [ ] Create test with large dataset
- [ ] Verify streaming behavior (constant memory)
- [ ] Set performance threshold (max time, max memory)
- [ ] Add to CI if critical path

**Example:**
```python
# python/tests/test_performance.py

def test_streaming_memory_constant():
    """Verify memory usage doesn't grow with dataset size."""
    import psutil
    import os

    process = psutil.Process(os.getpid())

    c = bundlebase.create()
    c.attach("tests/data/large_10gb.parquet")

    mem_before = process.memory_info().rss / 1024 / 1024  # MB

    # Process entire dataset
    df = c.to_pandas()

    mem_after = process.memory_info().rss / 1024 / 1024  # MB
    mem_growth = mem_after - mem_before

    # Memory growth should be small (< 100MB) regardless of dataset size
    assert mem_growth < 100, f"Memory grew by {mem_growth}MB - streaming violated!"
```

## Common Performance Issues

### 1. Using `collect()` Instead of Streaming

**Symptom:** Memory usage grows to 3x dataset size

**Example:**
```rust
// ❌ WRONG: 10GB file uses 30GB RAM
let batches = df.collect().await?;
process_all(batches);

// ✅ CORRECT: 10GB file uses 50MB RAM
let mut stream = df.execute_stream().await?;
while let Some(batch) = stream.next().await {
    process_batch(batch?);
}
```

**Fix:** Replace all `collect()` with `execute_stream()`

### 2. Accumulating Batches in Python

**Symptom:** Python `to_pandas()` runs out of memory

**Example:**
```python
# ❌ WRONG: Accumulates all batches
def to_pandas(self):
    batches = []
    for batch in self.stream_batches():
        batches.append(batch)  # Accumulating!
    return pa.Table.from_batches(batches).to_pandas()

# ✅ CORRECT: Streaming conversion (implemented in Rust)
def to_pandas(self):
    # Rust side handles streaming internally
    return self._inner.to_pandas()
```

**Fix:** Let Rust handle streaming, don't accumulate in Python

### 3. Reading All Columns When Only Few Needed

**Symptom:** Slow reads even with filters

**Example:**
```rust
// DataFusion automatically projects only needed columns
let df = ctx.read_parquet("file.parquet").await?;
let selected = df.select_columns(&["name", "age"])?;  // ✅ Only reads 2 columns
```

**Fix:** Trust DataFusion's projection pushdown (already optimized)

### 4. Multiple Passes Over Data

**Symptom:** Same data read multiple times

**Example:**
```rust
// ❌ WRONG: Two passes over data
let count = df.clone().count().await?;
let batches = df.collect().await?;

// ✅ CORRECT: Single pass with streaming
let mut stream = df.execute_stream().await?;
let mut count = 0;
while let Some(batch) = stream.next().await {
    count += batch?.num_rows();
    // Process batch here if needed
}
```

**Fix:** Combine operations into single streaming pass

### 5. Unnecessary Cloning of Large Data

**Symptom:** High CPU usage, slow performance

**Example:**
```rust
// ❌ WRONG: Expensive clone of large DataFrame
fn process(df: DataFrame) -> DataFrame {
    let copy = df.clone();  // Deep copy of entire plan
    copy.filter(...)
}

// ✅ CORRECT: Consume DataFrame (moves ownership)
fn process(df: DataFrame) -> DataFrame {
    df.filter(...)  // No clone needed
}
```

**Fix:** Use ownership moves instead of cloning

## Performance Validation Checklist

After optimization:

- [ ] Memory usage constant regardless of dataset size
- [ ] No `collect()` calls in hot paths
- [ ] All queries use `execute_stream()`
- [ ] Profiling shows improvement
- [ ] Tests pass with large datasets
- [ ] Documentation updated with performance characteristics

## Example: Fixing Slow `to_pandas()`

### 1. Issue
```python
# User reports: to_pandas() runs out of memory on 5GB file
df = c.to_pandas()  # ❌ Crashes
```

### 2. Investigation
```rust
// Found the problem in Rust implementation
pub async fn to_pandas(&self) -> Result<PyObject> {
    let df = self.to_dataframe().await?;
    let batches = df.collect().await?;  // 🚨 BUG: collect() loads all data
    // Convert batches to pandas...
}
```

### 3. Fix
```rust
pub async fn to_pandas(&self) -> Result<PyObject> {
    let df = self.to_dataframe().await?;
    let mut stream = df.execute_stream().await?;  // ✅ Streaming

    Python::with_gil(|py| {
        // Stream batches to pandas incrementally
        let pandas_list = PyList::empty(py);
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            let py_batch = batch_to_pyarrow(py, &batch)?;
            pandas_list.append(py_batch)?;
        }
        // Concatenate in pandas (streaming)
        pandas_concat(py, pandas_list)
    })
}
```

### 4. Validation
```python
# Before: 5GB file crashes (OOM)
# After: 5GB file uses ~50MB RAM ✅

def test_large_file():
    c = bundlebase.create()
    c.attach("5gb_file.parquet")
    df = c.to_pandas()  # Now works!
    assert len(df) > 0
```

## Performance Metrics

### Expected Performance

| Dataset Size | RAM Usage | Read Time (SSD) |
|--------------|-----------|-----------------|
| 100 MB       | ~50 MB    | ~1 second       |
| 1 GB         | ~50 MB    | ~3 seconds      |
| 10 GB        | ~50 MB    | ~30 seconds     |
| 100 GB       | ~50 MB    | ~5 minutes      |

**Key insight:** RAM usage constant, time scales linearly with data size.

### Warning Signs

- ⚠️ Memory usage grows with dataset size → streaming violation
- ⚠️ Time grows faster than linearly → inefficient algorithm
- ⚠️ CPU usage low but slow → I/O bottleneck
- ⚠️ Memory usage spikes then crashes → `collect()` call

## Success Criteria

Performance is acceptable when:

- ✅ Memory usage constant (<100MB) for any dataset size
- ✅ No `collect()` calls in production code paths
- ✅ Large dataset tests pass (>RAM size)
- ✅ Performance metrics documented
- ✅ Profiling shows no obvious bottlenecks
- ✅ Code review checklist complete

## Related Templates

- [fix-bug.md](fix-bug.md) - If performance issue is a bug
- [new-feature.md](new-feature.md) - Ensure new features are performant

## Related Documentation

- [decisions/003-streaming-only.md](../decisions/003-streaming-only.md) - Why streaming matters
- [architecture.md](../architecture.md#streaming-execution-architecture) - Streaming architecture
- [anti-patterns.md](../anti-patterns.md#section-1-streaming-execution) - Performance anti-patterns
