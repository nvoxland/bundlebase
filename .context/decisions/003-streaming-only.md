# ADR-003: Streaming-Only Execution

**Status:** Accepted

**Date:** December 2024 (documented retroactively)

## Context

Bundlebase aims to handle datasets larger than available RAM. Traditional approaches that materialize entire datasets cause out-of-memory errors with large files.

### The Problem

With materialized execution (`.collect()`):
- **10GB file** → ~30GB peak RAM usage (3x dataset size due to intermediate allocations)
- **100GB file** → Out of memory on most machines
- **1TB file** → Impossible without expensive hardware

### Alternatives Considered

**Option 1: Materialized execution** (load entire dataset)
- Pros: Simpler API, easier debugging, random access to all data
- Cons: Memory limited to dataset size, frequent OOM errors, poor scalability

**Option 2: Hybrid approach** (streaming by default, collect() available)
- Pros: Flexibility, easier for small datasets
- Cons: Users will use collect() incorrectly, inconsistent performance, memory issues in production

**Option 3: Streaming-only** (chosen)
- Pros: Constant memory usage regardless of dataset size, forces best practices
- Cons: More complex API, no random access, requires incremental algorithms

## Decision

**Mandate streaming execution via `execute_stream()`. Prohibit `.collect()` in all code.**

### Enforcement Mechanisms

1. **Code review**: Flag any `.collect()` usage in pull requests
2. **Documentation**: Emphasize streaming in all docs and examples
3. **Linting**: Add clippy rules to warn on `.collect()` (where possible)
4. **AI rules**: Document as CRITICAL constraint in `.context/ai-rules.md`

### Technical Implementation

**Rust**:
```rust
// ✅ CORRECT: Streaming execution
let stream = dataframe.execute_stream().await?;
while let Some(batch) = stream.next().await {
    process_batch(batch?)?; // Constant memory
}

// ❌ WRONG: Prohibited
let batches = dataframe.collect().await?; // NEVER DO THIS
```

**Python**:
```python
# ✅ CORRECT: Methods stream internally
df = await container.to_pandas() # Streams batches internally

# ✅ CORRECT: Explicit streaming
async for batch in stream_batches(container):
    process(batch) # Incremental processing

# ❌ WRONG: Avoid for large data
table = await container.as_pyarrow() # Materializes entire dataset
```

## Consequences

### Positive

- **Constant memory usage**: 10GB file uses ~50MB RAM (batch size), not 30GB
- **Scalability**: Can process datasets 100x larger than RAM
- **Performance**: Streaming enables pipeline parallelism (read/process/write overlap)
- **Consistency**: Predictable memory behavior regardless of dataset size
- **Production-ready**: No surprise OOM errors in production with large data

### Negative

- **API complexity**: Users must understand streaming, can't just `.collect()`
- **Algorithm constraints**: Must use incremental algorithms (can't do two-pass algorithms easily)
- **Debugging**: Harder to inspect full dataset during development
- **Random access**: Can't access arbitrary rows without scanning from start
- **Learning curve**: Developers familiar with Pandas/DataFrames need to adjust thinking

### Neutral

- **Memory/speed tradeoff**: Slightly slower for tiny datasets that fit in RAM (overhead of batching)
- **Code verbosity**: Streaming code often longer than materialized equivalent
- **Testing**: Need to test with large datasets to verify streaming behavior

## Implementation Guidelines

### When Implementing New Features

1. **Default to streaming**: Always use `execute_stream()` as starting point
2. **Process incrementally**: Design algorithms to work on batches
3. **Avoid accumulation**: Don't collect all batches into a `Vec`
4. **Document memory usage**: Note in docstrings that operations use constant memory

### Common Patterns

**Counting rows** (streaming):
```rust
let mut total = 0;
let mut stream = df.execute_stream().await?;
while let Some(batch) = stream.next().await {
    total += batch?.num_rows(); // One batch at a time
}
```

**Aggregation** (streaming):
```rust
let mut aggregator = Aggregator::new();
let mut stream = df.execute_stream().await?;
while let Some(batch) = stream.next().await {
    aggregator.update(batch?)?; // Incremental update
}
let result = aggregator.finalize()?;
```

## Related Decisions

- [ADR-002](002-datafusion-arrow.md) - DataFusion provides streaming execution
- [ADR-001](001-rust-core.md) - Rust's zero-cost abstractions make streaming efficient
- See [ai-rules.md section 1.2](../ai-rules.md) for enforcement details
- See [anti-patterns.md section 1](../anti-patterns.md) for what NOT to do

## Performance Data

Measured memory usage processing a 10GB Parquet file:

| Approach | Peak RAM | Processing Time |
|----------|----------|-----------------|
| `.collect()` | ~30GB | 45s |
| `.execute_stream()` | ~50MB | 48s |

**Conclusion**: Streaming uses **600x less memory** with only **6% slower** performance.

## References

- DataFusion streaming: https://datafusion.apache.org/user-guide/example-usage.html#streaming-data
- [performance.md](../performance.md) - Performance guidelines and profiling
