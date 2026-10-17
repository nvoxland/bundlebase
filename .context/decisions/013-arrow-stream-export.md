# ADR-013: Standard Arrow Stream Export

**Status:** Accepted

**Date:** October 2026

## Context

Bundlebase already executes queries as Arrow batch streams, but its Python async
iterator cannot be registered directly with synchronous Arrow consumers such as
DuckDB. Materializing every batch into a table defeats streaming execution.
The bridge must preserve ownership, allow Python callbacks during query
execution, and survive the creating asyncio loop closing.

## Decision

Expose explicit async `arrow_reader()` methods on bundles, builders, and query
results, with matching synchronous wrappers. Return a standard
`pyarrow.RecordBatchReader` using Arrow's C Stream interface. Keep query selection
in `query(sql, params)` and treat reader creation as a terminal chain conversion.
Do not add an implicit Arrow stream protocol to bundle objects.

A Rust-owned synchronous adapter polls the existing stream using the shared
Tokio runtime. Reading waits without holding the Python GIL. Do not create a
runtime or producer thread per reader, accumulate batches, or bridge through a
Python async callback for each batch. Transfer stream ownership atomically,
including any peeked batch. Keep query resources alive until close/drop, and
release them without draining the remaining stream.

Each bundle or builder export prepares a fresh stream. A query result can be
exported once; subsequent conversions fail with its consumed-result error.
Readers are sequential and non-replayable, with normal Arrow EOF behavior.

DuckDB is a development/test dependency and an optional consumer. Bundlebase
SQL remains responsible for selecting columns and predicates before export.
An issued Arrow stream does not provide source predicate or projection
pushdown from DuckDB. Investigate supported upstream scan interfaces separately
before designing a dataset API; accepted filters must always be fully enforced.

## Consequences

- Standard Arrow consumers can read results without full materialization.
- The Rust stream and its resources outlive Python wrappers and asyncio loops.
- Users must recreate readers or explicitly materialize data for repeated scans.
- The bridge adds runtime, GIL, ownership, and early-close checks at the FFI boundary.
- Inbound Arrow attachment and external row updates remain separate features.

## Decision History

The initial proposal coupled reader export with source pushdown. The reader
ships independently because the Arrow C Stream interface carries batches, not
upstream scan requests. DuckDB filters on an issued stream do not prove reduced
Bundlebase source reads. Unsupported future scan predicates must use a proven
residual path or fail explicitly; they cannot be silently discarded.

The executable investigation in `scripts/duckdb-pushdown-spike.py` used DuckDB
1.5.6 and PyArrow 23.0.1. Capsule scans received no requested schema or source
predicates, and DuckDB rejected a duck-typed Dataset. An isolated experimental
subclass bypassed native Dataset initialization to observe scan requests; that
unsupported construction is not a production API. Dropping its accepted filter
returned incorrect aggregates because the consumer did not reapply it.

The experiment counts logical source batch and column requests, not physical
file IO. Selective consumer queries still requested all source batches; applying
an Arrow scanner filter after reading did not establish source pruning. The
recommendation is no-go for a production `arrow_dataset()` on these interfaces.
A future design needs a supported scan hook, complete predicate enforcement,
and measured physical source IO reduction before claiming source pushdown.
