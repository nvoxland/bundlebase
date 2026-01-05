# ADR-002: DataFusion and Apache Arrow

**Status:** Accepted

**Date:** December 2024 (documented retroactively)

## Context

Bundlebase needed:
- **SQL query engine** for executing user queries and transformations
- **Columnar data format** for efficient analytics and memory usage
- **Streaming execution** to handle datasets larger than RAM
- **Cross-language compatibility** for Python integration

## Decision

**Use Apache DataFusion for SQL execution and Apache Arrowfor data representation.**

### Implementation Approach

- **DataFusion** (`SessionContext`) executes SQL queries, manages optimizations, and provides execution plans
- **Arrow** provides columnar `RecordBatch` format, schema definitions, and FFI compatibility
- **Streaming execution** via `execute_stream()` returns `SendableRecordBatchStream`
- **Python integration** via PyArrow FFI for zero-copy data sharing

### Version Pinning

- DataFusion: v51 (workspace dependency)
- Arrow: v57 (workspace dependency)
- Arrow-Flight: v57 (for potential future RPC support)

Versions are pinned in `Cargo.toml` workspace dependencies for consistency across crates.

## Consequences

### Positive

- **Mature SQL engine**: Comprehensive SQL support (JOIN, GROUP BY, window functions, CTEs)
- **Streaming architecture**: Built for datasets larger than RAM via `execute_stream()`
- **Query optimization**: Cost-based optimizer, predicate pushdown, projection pushdown
- **Columnar format**: Arrow's columnar layout ideal for analytics (SIMD vectorization, compression)
- **Zero-copy FFI**: Arrow FFI enables efficient Rust ↔ Python data transfer
- **Active ecosystem**: Apache Arrow used by Pandas, Polars, DuckDB (interoperability)
- **Rust-native**: No C++ FFI complexity, integrates naturally with Rust codebase

### Negative

- **Large dependency**: Arrow + DataFusion add ~50MB to binary size
- **Compilation time**: DataFusion increases build times significantly (~2-3 minutes)
- **API stability**: DataFusion pre-1.0 (breaking changes between versions)
- **Complexity**: Understanding query planning and optimization requires deep knowledge
- **Memory overhead**: Arrow's chunked arrays have some memory overhead vs packed arrays

### Neutral

- **Learning curve**: Contributors need to understand DataFusion's execution model
- **Version upgrades**: Need to track DataFusion releases for bug fixes and optimizations
- **Arrow compatibility**: Must match PyArrow version on Python side (currently 14.0.0+)

## Related Decisions

- [ADR-001](001-rust-core.md) - Rust enables natural DataFusion integration
- [ADR-003](003-streaming-only.md) - DataFusion's streaming execution is core to this choice
- [ADR-006](006-lazy-evaluation.md) - DataFusion's lazy evaluation model influences design

## Technical Details

### DataFusion Usage

```rust
use datafusion::prelude::*;

// Create session context
let ctx = SessionContext::new();

// Register table
ctx.register_table("my_table", table_provider)?;

// Execute query with streaming
let df = ctx.sql("SELECT * FROM my_table WHERE age > 18").await?;
let stream = df.execute_stream().await?; // Streaming!
```

### Arrow FFI to Python

```rust
use pyo3::prelude::*;
use arrow::ffi_stream::*;

// Export Arrow stream to Python
let stream = PyRecordBatchStream::new(df.execute_stream().await?, schema);
stream.into_py(py) // Zero-copy to PyArrow
```

## References

- **DataFusion**: https://datafusion.apache.org/
- **Apache Arrow**: https://arrow.apache.org/
- **Arrow FFI**: https://arrow.apache.org/docs/format/CDataInterface.html
- **See also**: [architecture.md](../architecture.md#streaming-execution-architecture)
