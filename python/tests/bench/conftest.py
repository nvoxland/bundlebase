"""Shared fixtures and utilities for benchmark tests.

These benchmarks use pytest-benchmark for timing measurements.
Run with: poetry run pytest python/tests/bench/ --benchmark-only
"""

import os

# CRITICAL: Set CARGO_TARGET_DIR before any imports that might trigger maturin_import_hook
os.environ['CARGO_TARGET_DIR'] = 'target/maturin'

import asyncio
import random
import tempfile
from pathlib import Path

import pyarrow as pa
import pyarrow.parquet as pq
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


def create_temp_parquet(rows: int, seed: int = 42) -> tuple[str, str]:
    """Generate synthetic data and write to a temporary parquet file.

    Returns (file_url, file_path) where file_url is a file:// URL for attach()
    and file_path is the raw path for cleanup.
    """
    batch = generate_benchmark_data(rows, seed)
    table = pa.Table.from_batches([batch])
    f = tempfile.NamedTemporaryFile(suffix='.parquet', delete=False)
    pq.write_table(table, f.name)
    f.close()
    return Path(f.name).as_uri(), f.name


async def create_bundle_with_data(rows: int, seed: int = 42):
    """Create a bundle with synthetic benchmark data.

    Returns an OperationChain wrapping a PyBundleBuilder.
    """
    data_url, _ = create_temp_parquet(rows, seed)
    c = await bundlebase.create(bundlebase.random_memory_url())
    c = await c.attach(data_url)
    c = await c.commit("Benchmark setup")
    return c


@pytest.fixture
def event_loop():
    """Create event loop for async tests."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


@pytest.fixture(scope="session")
def data_path_1k():
    """file:// URL to temporary parquet file with 1K rows of synthetic data."""
    file_url, file_path = create_temp_parquet(SCALE_1K)
    yield file_url
    os.unlink(file_path)


@pytest.fixture(scope="session")
def data_path_10k():
    """file:// URL to temporary parquet file with 10K rows of synthetic data."""
    file_url, file_path = create_temp_parquet(SCALE_10K)
    yield file_url
    os.unlink(file_path)


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
