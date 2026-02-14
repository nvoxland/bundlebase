# Benchmarking Guide

This document describes how to run and use the benchmarking infrastructure for Bundlebase.

## Overview

Bundlebase includes comprehensive benchmarks for both Rust and Python to measure performance of core operations. The benchmarks work **completely offline** using `throttle://` (Rust) and `memory://` (Python) storage (no S3/cloud access required).

**Key Constraint:** Bundlebase targets ~50MB constant memory regardless of dataset size. The streaming benchmarks verify this property.

## Rust Benchmarks

Rust benchmarks use [Criterion](https://github.com/bheisler/criterion.rs) for statistical rigor and HTML reports.

### Running Rust Benchmarks

```bash
# Run all Rust benchmarks
cargo bench --package bundlebase

# Run a specific benchmark file
cargo bench --package bundlebase --bench bundle_lifecycle
cargo bench --package bundlebase --bench query_execution
cargo bench --package bundlebase --bench index_operations
cargo bench --package bundlebase --bench streaming

# Run benchmarks matching a name pattern
cargo bench --package bundlebase -- create_empty
cargo bench --package bundlebase -- filter_selective

# Compile benchmarks without running (useful for CI)
cargo bench --package bundlebase --no-run
```

### Viewing Results

After running benchmarks, Criterion generates an HTML report:

```bash
open target/criterion/report/index.html
```

### Available Rust Benchmarks

| File | Benchmarks | Description |
|------|------------|-------------|
| `bundle_lifecycle.rs` | `create_empty_bundle`, `attach_data` | Bundle creation and data attachment |
| `query_execution.rs` | `filter_selective`, `filter_broad`, `aggregation_sum`, `filter_parameterized`, `join_small_large`, `projection` | Query operations at various scales |
| `index_operations.rs` | `create_index`, `index_lookup_exact`, `index_vs_scan`, `index_range_query`, `index_in_query` | Index creation and query acceleration |
| `streaming.rs` | `stream_rows`, `stream_with_filter`, `stream_with_aggregation`, `stream_projection`, `stream_1m_rows` | Memory-efficient streaming verification |

### Benchmark Scales

The benchmarks test at multiple data scales:

| Scale | Rows | Use Case |
|-------|------|----------|
| `SCALE_1K` | 1,000 | Micro-benchmarks |
| `SCALE_10K` | 10,000 | Small datasets |
| `SCALE_100K` | 100,000 | Medium datasets |
| `SCALE_1M` | 1,000,000 | Large datasets, memory verification |

## Python Benchmarks

Python benchmarks use [pytest-benchmark](https://pytest-benchmark.readthedocs.io/) which integrates with the existing pytest infrastructure.

### Running Python Benchmarks

```bash
# Run all Python benchmarks
poetry run pytest python/tests/bench/ --benchmark-only

# Run specific benchmark file
poetry run pytest python/tests/bench/test_lifecycle.py --benchmark-only
poetry run pytest python/tests/bench/test_conversions.py --benchmark-only
poetry run pytest python/tests/bench/test_memory.py --benchmark-only

# Verbose output with individual test times
poetry run pytest python/tests/bench/ --benchmark-only -v

# Export results to JSON for analysis
poetry run pytest python/tests/bench/ --benchmark-only --benchmark-json=results.json

# Compare against previous run
poetry run pytest python/tests/bench/ --benchmark-only --benchmark-compare

# Save baseline for future comparison
poetry run pytest python/tests/bench/ --benchmark-only --benchmark-save=baseline
```

### Available Python Benchmarks

| File | Tests | Description |
|------|-------|-------------|
| `test_lifecycle.py` | 7 tests | Bundle creation, attach, commit, filter, select |
| `test_conversions.py` | 7 tests | to_pandas, to_polars, stream_batches, as_pyarrow |
| `test_memory.py` | 6 tests | Memory usage verification, streaming memory |

## Benchmark Structure

```
rust/bundlebase/
├── benches/
│   ├── data_generator.rs     # Synthetic data generation
│   ├── bench_data.rs         # Cached data file management
│   ├── bench_helpers.rs      # Shared benchmark utilities
│   ├── throttled_store.rs    # Throttled storage for benchmarks
│   ├── bundle_lifecycle.rs   # Bundle operations
│   ├── query_execution.rs    # Query benchmarks
│   ├── index_operations.rs   # Index benchmarks
│   └── streaming.rs          # Memory/streaming benchmarks

python/tests/bench/
├── conftest.py               # Shared fixtures
├── test_lifecycle.py         # Lifecycle benchmarks
├── test_conversions.py       # Conversion benchmarks
└── test_memory.py            # Memory benchmarks
```

## Synthetic Data Generation

Benchmarks generate reproducible test data at runtime (not stored in repo):

```rust
pub struct BenchmarkDataConfig {
    pub rows: usize,      // 1K to 1M
    pub seed: u64,        // For reproducibility (default: 42)
}
```

Generated columns:
- `id` - Sequential integers (for index testing)
- `filter_value` - Random 0-99 (for filter selectivity testing)
- `amount` - Random floats (for aggregation)
- `category` - Random A-E (for GROUP BY)
- `name` - Sequential strings
- `region` - Random North/South/East/West (for joins)

## Interpreting Results

### Criterion (Rust)

Criterion reports:
- **Mean** - Average execution time
- **StdDev** - Standard deviation
- **Median** - Middle value (less affected by outliers)
- **Throughput** - Operations per second or elements per second

Look for:
- Consistent results across runs (low StdDev)
- Performance scaling with data size
- No unexpected regressions

### pytest-benchmark (Python)

pytest-benchmark reports:
- **Min/Max/Mean** - Time statistics
- **StdDev** - Variation
- **OPS** - Operations per second
- **Rounds** - Number of benchmark iterations

## CI Integration

Benchmarks are **manual-only** by default (they're slow and hardware-dependent).

To compile-check benchmarks in CI:

```yaml
# GitHub Actions example
- name: Check benchmarks compile
  run: cargo bench --package bundlebase --no-run
```

For optional weekly regression tracking:

```yaml
# Run benchmarks and save results
- name: Run benchmarks
  run: |
    cargo bench --package bundlebase -- --save-baseline main
    poetry run pytest python/tests/bench/ --benchmark-only --benchmark-json=results.json
```

## Adding New Benchmarks

### Rust

1. Add benchmark function to appropriate file in `benches/`
2. Use `data_generator` for synthetic data
3. Use `criterion_group!` to register the benchmark

```rust
fn bench_my_operation(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");

    c.bench_function("my_operation", |b| {
        b.to_async(&rt).iter(|| async {
            // Benchmark code here
        });
    });
}

criterion_group!(benches, bench_my_operation);
```

### Python

1. Add test function to appropriate file in `python/tests/bench/`
2. Use `event_loop` fixture for async operations
3. Use `benchmark` fixture from pytest-benchmark

```python
def test_my_operation(benchmark, event_loop):
    async def my_operation():
        c = await bundlebase.create(bundlebase.random_memory_url())
        # Operation code here
        return result

    def run():
        return event_loop.run_until_complete(my_operation())

    result = benchmark(run)
    assert result is not None
```
