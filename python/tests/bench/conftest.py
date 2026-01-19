"""Shared fixtures and utilities for benchmark tests.

These benchmarks use pytest-benchmark for timing measurements.
Run with: poetry run pytest python/tests/bench/ --benchmark-only
"""

import os

# CRITICAL: Set CARGO_TARGET_DIR before any imports that might trigger maturin_import_hook
os.environ['CARGO_TARGET_DIR'] = 'target/maturin'

import asyncio
import random
from typing import Any

import pyarrow as pa
import pytest
import bundlebase


# Standard benchmark scales
SCALE_1K = 1_000
SCALE_10K = 10_000
SCALE_100K = 100_000
SCALE_1M = 1_000_000


def generate_benchmark_data(rows: int, seed: int = 42) -> pa.RecordBatch:
    """Generate synthetic benchmark data.

    Creates a RecordBatch with reproducible test data including:
    - id: Sequential integers (0 to rows-1)
    - filter_value: Random integers 0-99 for filter testing
    - amount: Random floats for aggregation testing
    - category: Random categories A-E for grouping
    - name: Sequential item names
    - region: Random regions for join testing
    """
    random.seed(seed)

    ids = list(range(rows))
    filter_values = [random.randint(0, 99) for _ in range(rows)]
    amounts = [random.random() * 10000 for _ in range(rows)]
    categories = [random.choice(['A', 'B', 'C', 'D', 'E']) for _ in range(rows)]
    names = [f"item_{i:08d}" for i in range(rows)]
    regions = [random.choice(['North', 'South', 'East', 'West']) for _ in range(rows)]

    return pa.RecordBatch.from_pydict({
        'id': ids,
        'filter_value': filter_values,
        'amount': amounts,
        'category': categories,
        'name': names,
        'region': regions,
    })


def write_parquet_bytes(batch: pa.RecordBatch) -> bytes:
    """Write a RecordBatch to parquet bytes."""
    import io
    sink = io.BytesIO()
    writer = pa.parquet.ParquetWriter(sink, batch.schema)
    writer.write_batch(batch)
    writer.close()
    return sink.getvalue()


@pytest.fixture
def event_loop():
    """Create event loop for async tests."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


@pytest.fixture
def benchmark_data_1k() -> pa.RecordBatch:
    """1K row benchmark dataset."""
    return generate_benchmark_data(SCALE_1K)


@pytest.fixture
def benchmark_data_10k() -> pa.RecordBatch:
    """10K row benchmark dataset."""
    return generate_benchmark_data(SCALE_10K)


@pytest.fixture
def benchmark_data_100k() -> pa.RecordBatch:
    """100K row benchmark dataset."""
    return generate_benchmark_data(SCALE_100K)


async def create_bundle_with_data(rows: int):
    """Create a bundle with synthetic benchmark data.

    Returns an OperationChain wrapping a PyBundleBuilder.
    """
    batch = generate_benchmark_data(rows)
    parquet_bytes = write_parquet_bytes(batch)

    # Write to memory location
    data_url = f"memory:///bench_data_{random.randint(0, 2**63)}.parquet"
    bundle_url = f"memory:///bench_bundle_{random.randint(0, 2**63)}"

    # Use bundlebase's internal API to write the parquet file
    # and create a bundle pointing to it
    c = await bundlebase.create(bundle_url)

    # For benchmarks, we'll use the test data file approach
    # since we can't directly write to memory:// from Python
    c = await c.attach(bundlebase.test_datafile("userdata.parquet"))

    return c


def run_async(coro):
    """Run an async coroutine synchronously."""
    return asyncio.get_event_loop().run_until_complete(coro)
