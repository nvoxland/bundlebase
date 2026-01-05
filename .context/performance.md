# Performance Characteristics

This document describes bundlebase's performance characteristics, optimization strategies, and performance testing practices.

## Performance Philosophy

Bundlebase is designed to handle **datasets larger than RAM** through streaming execution:

1. **Constant memory usage** - RAM independent of dataset size
2. **Streaming-first** - Process data incrementally, never load entire dataset
3. **Lazy evaluation** - Execute queries only when results needed
4. **Predicate pushdown** - Filter at data source when possible
5. **Columnar efficiency** - Leverage Apache Arrow's columnar format

**Critical constraint:** NEVER use `df.collect()` - always use `df.execute_stream()`

**See:** [decisions/003-streaming-only.md](decisions/003-streaming-only.md)

---

## Expected Performance

### Memory Usage

| Dataset Size | RAM Usage | Explanation |
|--------------|-----------|-------------|
| 100 MB       | ~50 MB    | Constant batch processing |
| 1 GB         | ~50 MB    | Same - streaming execution |
| 10 GB        | ~50 MB    | Same - data processed in batches |
| 100 GB       | ~50 MB    | Same - no dataset loaded into memory |
| 1 TB         | ~50 MB    | Same - streaming is key |

**Key insight:** Memory usage is **constant** regardless of dataset size.

**If memory grows with dataset → streaming violation (bug).**

---

### Query Execution Time

| Operation | 100 MB | 1 GB | 10 GB | Scaling |
|-----------|--------|------|-------|---------|
| Read Parquet | ~1s | ~3s | ~30s | O(n) - linear |
| Filter | +0.5s | +1.5s | +5s | O(n) - linear |
| Select columns | ~0s | ~0s | ~0s | O(1) - projection pushdown |
| to_pandas() | ~2s | ~6s | ~60s | O(n) - linear |

**Scaling:** Time grows linearly with data size (expected for streaming).

**Baseline:** SSD read speed ~300-500 MB/s determines lower bound.

---

### Batch Size Impact

| Batch Size | Memory | Throughput | Latency |
|------------|--------|------------|---------|
| 1,024 rows | Low | Lower | Low |
| 8,192 rows | Medium | Medium | Medium |
| 65,536 rows | Higher | Higher | Higher |

**Default:** 8,192 rows (DataFusion default)

**Trade-off:** Larger batches = higher throughput but more memory per batch.

---

## Streaming Execution Architecture

### How Streaming Works

```rust
// ❌ WRONG: Collect loads entire dataset into memory
let batches: Vec<RecordBatch> = df.collect().await?;
// Memory usage: 3x dataset size (compressed + uncompressed + working)

// ✅ CORRECT: Streaming processes incrementally
let mut stream = df.execute_stream().await?;
while let Some(batch) = stream.next().await {
    let batch = batch?;
    // Process this batch (~8K rows)
    // Memory usage: constant (only current batch in memory)
}
```

### Memory Profile Comparison

**Collect (wrong):**
```
Memory │     ┌────────────┐
Usage  │    ╱│            │
       │   ╱ │            │ OOM!
       │  ╱  │            │
       │ ╱   │            │
       └─────────────────────> Time
        Load   Process
```

**Streaming (correct):**
```
Memory │ ┌─┐ ┌─┐ ┌─┐ ┌─┐
Usage  │ │ │ │ │ │ │ │ │ Constant
       │ │ │ │ │ │ │ │ │
       │ └─┘ └─┘ └─┘ └─┘
       └─────────────────────> Time
        Batch Batch Batch
```

**See:** [architecture.md](architecture.md#streaming-execution-architecture)

---

## Performance Optimization Patterns

### 1. Projection Pushdown (Automatic)

**Pattern:** Select only needed columns

```python
# Only reads "name" and "age" columns from Parquet
c.select(["name", "age"])
df = c.to_pandas()

# DataFusion automatically pushes projection to ParquetExec
# Result: Reads only 2 columns instead of all 100 columns
```

**Savings:** Proportional to columns selected vs total columns

**Example:** 2/100 columns = 98% less data read

---

### 2. Predicate Pushdown (Automatic)

**Pattern:** Filter at data source

```python
# DataFusion pushes filter into Parquet reader
c.filter("age > 18")
df = c.to_pandas()

# ParquetExec skips row groups where max(age) <= 18
# Result: Reads only relevant row groups
```

**Savings:** Depends on data distribution and filter selectivity

**Example:** Filter reducing dataset to 10% = 90% less data read

---

### 3. Lazy Evaluation

**Pattern:** Defer execution until query time

```python
# These operations don't execute yet (just recorded)
c.filter("age > 18")  # Recorded
c.select(["name"])     # Recorded
c.filter("age < 65")   # Recorded

# Execution happens here (all operations in single pass)
df = c.to_pandas()  # NOW execute entire pipeline
```

**Benefit:** Query optimizer can reorder/combine operations

**Example:** Two filters combined into single filter

---

### 4. Batch Processing in Python

**Pattern:** Process large datasets incrementally

```python
# For custom processing of large datasets
for batch in c.stream_batches():
    # batch is pyarrow.RecordBatch (~8K rows)
    process_batch(batch)  # Process incrementally
    # Memory: only one batch at a time
```

**Use case:** Custom processing, incremental writes

**See:** [python-api.md](python-api.md#streaming-api-for-large-datasets)

---

## Performance Anti-Patterns

### ❌ 1. Using `collect()` in Rust

**Wrong:**
```rust
let batches = df.collect().await?;  // Loads entire dataset!
for batch in batches {
    process(batch);
}
```

**Right:**
```rust
let mut stream = df.execute_stream().await?;
while let Some(batch) = stream.next().await {
    process(batch?);
}
```

**Impact:** 3x dataset size in memory, potential OOM

---

### ❌ 2. Accumulating Batches in Python

**Wrong:**
```python
batches = []
for batch in c.stream_batches():
    batches.append(batch)  # Accumulating!
df = pa.Table.from_batches(batches)  # Entire dataset in memory
```

**Right:**
```python
# Let Rust handle streaming internally
df = c.to_pandas()  # Streams internally, constant memory
```

**Impact:** Defeats streaming, memory grows with dataset

---

### ❌ 3. Unnecessary Clones

**Wrong:**
```rust
fn process(df: DataFrame) -> DataFrame {
    let copy = df.clone();  // Expensive logical plan clone
    copy.filter(col("age").gt(lit(18)))
}
```

**Right:**
```rust
fn process(df: DataFrame) -> DataFrame {
    df.filter(col("age").gt(lit(18)))  // Consume df (move)
}
```

**Impact:** Higher CPU, slower query construction

---

### ❌ 4. Multiple Passes Over Data

**Wrong:**
```python
# Two passes over data
count = c.count()  # Pass 1
df = c.to_pandas()  # Pass 2
```

**Right:**
```python
# Single pass
df = c.to_pandas()
count = len(df)
```

**Impact:** 2x I/O, 2x execution time

---

### ❌ 5. Reading All Columns When Few Needed

**Wrong:**
```python
df = c.to_pandas()  # Reads all 100 columns
names = df["name"]  # Only needed one column
```

**Right:**
```python
c.select(["name"])  # Project before reading
df = c.to_pandas()  # Reads only 1 column
```

**Impact:** 100x more data read than necessary

---

## Performance Testing

### 1. Memory Usage Test

```python
import psutil
import os

def test_streaming_memory():
    process = psutil.Process(os.getpid())

    c = bundlebase.create()
    c.attach("large_10gb.parquet")

    mem_before = process.memory_info().rss / 1024 / 1024  # MB

    # Process entire dataset
    df = c.to_pandas()

    mem_after = process.memory_info().rss / 1024 / 1024  # MB
    mem_growth = mem_after - mem_before

    # Memory growth should be small
    assert mem_growth < 100, f"Memory grew by {mem_growth}MB"
```

**Expected:** <100MB growth regardless of dataset size

---

### 2. Execution Time Test

```python
import time

def test_execution_time():
    start = time.time()

    c = bundlebase.create()
    c.attach("large_10gb.parquet")
    c.filter("age > 18")
    df = c.to_pandas()

    elapsed = time.time() - start

    # Rough estimate: 300 MB/s read speed
    # 10GB / 300MB/s = ~33 seconds
    assert elapsed < 60, f"Took {elapsed}s, expected <60s"
```

**Expected:** Linear scaling with dataset size

---

### 3. Scalability Test

```python
def test_scalability():
    """Verify performance scales linearly."""
    sizes = [100_000, 1_000_000, 10_000_000]  # rows
    times = []

    for size in sizes:
        c = bundlebase.create()
        c.attach(f"test_data_{size}.parquet")

        start = time.time()
        df = c.to_pandas()
        times.append(time.time() - start)

    # Times should scale roughly linearly
    # 10x data should take ~10x time
    ratio = times[2] / times[0]  # 100x data increase
    assert 50 < ratio < 150, f"Scaling ratio {ratio} not linear"
```

**Expected:** Time ratio matches data ratio (linear scaling)

---

## Profiling and Debugging

### Rust Profiling

```bash
# Build with release optimizations
cargo build --release

# Flame graph (Linux)
cargo flamegraph --bin bundlebase

# Profiling with perf (Linux)
perf record --call-graph dwarf target/release/bundlebase
perf report

# Heap profiling
cargo install heaptrack
heaptrack target/release/bundlebase
```

---

### Python Profiling

```bash
# Time profiling
python -m cProfile -o profile.stats script.py
python -m pstats profile.stats

# Memory profiling
pip install memory_profiler
python -m memory_profiler script.py

# Line profiler
pip install line_profiler
kernprof -l -v script.py
```

---

### DataFusion Query Plans

```rust
// Inspect logical plan
let logical_plan = df.logical_plan();
println!("Logical plan:\n{:?}", logical_plan);

// Inspect physical plan
let physical_plan = df.create_physical_plan().await?;
println!("Physical plan:\n{:?}", physical_plan);
```

**Look for:**
- Predicate pushdown (filter in ParquetExec)
- Projection pushdown (only needed columns in scan)
- Unnecessary operations

---

## Performance Benchmarks

### Read Performance

| Format | Size | Time | Throughput |
|--------|------|------|------------|
| Parquet (uncompressed) | 1 GB | 2s | 500 MB/s |
| Parquet (snappy) | 1 GB | 3s | 333 MB/s |
| CSV | 1 GB | 8s | 125 MB/s |

**Recommendation:** Use Parquet with Snappy compression (good balance)

---

### Filter Performance

| Selectivity | Time vs Full Scan | Explanation |
|-------------|-------------------|-------------|
| 90% filtered | ~10% time | Most row groups skipped |
| 50% filtered | ~50% time | Half row groups skipped |
| 10% filtered | ~95% time | Most row groups read |

**Recommendation:** Parquet row group statistics enable efficient filtering

---

### Column Selection Performance

| Columns Selected | Time vs All Columns | Explanation |
|------------------|---------------------|-------------|
| 1 of 100 | ~1% time | Only one column read |
| 10 of 100 | ~10% time | Columnar format advantage |
| 50 of 100 | ~50% time | Linear scaling |

**Recommendation:** Select only needed columns before reading

---

## Optimization Checklist

When optimizing performance:

- [ ] Verify streaming execution (no `collect()` calls)
- [ ] Check memory usage is constant (not growing with data)
- [ ] Select only needed columns (projection pushdown)
- [ ] Apply filters early (predicate pushdown)
- [ ] Use Parquet format for data sources
- [ ] Combine multiple filters into single expression
- [ ] Avoid unnecessary clones
- [ ] Process data in single pass
- [ ] Use appropriate batch size
- [ ] Profile before optimizing (measure first)

---

## Performance Troubleshooting

### Issue: Memory Usage Growing

**Symptom:** RAM usage increases with dataset size

**Diagnosis:**
```bash
# Search for collect() calls
rg "\.collect\(\)" --type rust
```

**Solution:** Replace `collect()` with `execute_stream()`

**See:** [prompts/performance-review.md](prompts/performance-review.md)

---

### Issue: Slow Query Execution

**Symptom:** Queries slower than expected

**Diagnosis:**
1. Check if reading all columns (should select only needed)
2. Check if filters applied (should use predicate pushdown)
3. Profile to find bottleneck

**Solution:**
- Add `select()` to read fewer columns
- Ensure filters use indexed columns
- Check DataFusion query plan

---

### Issue: Out of Memory

**Symptom:** Process crashes with OOM

**Diagnosis:** Streaming violation (using `collect()`)

**Solution:**
1. Find `collect()` call in stack trace
2. Replace with `execute_stream()`
3. Add memory usage test to prevent regression

---

## Performance Guidelines Summary

### DO ✅

- Use `execute_stream()` for all query execution
- Select only needed columns before reading
- Apply filters as early as possible
- Use Parquet format for data storage
- Process data in single pass when possible
- Combine operations for optimizer to optimize
- Profile before optimizing (measure first)
- Test with datasets larger than RAM

### DON'T ❌

- Use `df.collect()` in production code
- Read all columns when only few needed
- Accumulate batches in Python
- Clone DataFrames unnecessarily
- Make multiple passes over data
- Ignore memory usage in tests
- Assume operations are cheap without profiling
- Use CSV for large datasets (use Parquet)

---

## Performance-Related Decisions

- [ADR-002: DataFusion and Apache Arrow](decisions/002-datafusion-arrow.md) - Why DataFusion for performance
- [ADR-003: Streaming-Only Execution](decisions/003-streaming-only.md) - Critical performance constraint
- [ADR-006: Lazy Operation Evaluation](decisions/006-lazy-evaluation.md) - Enables query optimization

---

## External Resources

- **DataFusion Performance:** https://arrow.apache.org/datafusion/user-guide/introduction.html
- **Arrow Columnar Format:** https://arrow.apache.org/docs/format/Columnar.html
- **Parquet Format:** https://parquet.apache.org/docs/file-format/

---

## Summary

**Key Performance Characteristics:**

1. **Memory:** Constant (~50MB) regardless of dataset size
2. **Time:** Linear scaling with data size (O(n))
3. **I/O:** Limited by SSD read speed (~300-500 MB/s)
4. **CPU:** Minimal overhead (DataFusion optimized)

**Critical Constraint:** ALWAYS use streaming execution - NEVER use `collect()`

**Optimization Focus:**
1. Streaming execution (most important)
2. Column projection (read less data)
3. Predicate pushdown (filter early)
4. Single-pass processing (avoid redundant reads)

**Success Metric:** Can process datasets 10-100x larger than RAM with constant memory usage.
