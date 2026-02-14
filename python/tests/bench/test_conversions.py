"""Data conversion benchmarks.

Benchmarks for converting bundle data to various formats (pandas, polars, pyarrow).

Run with: poetry run pytest python/tests/bench/test_conversions.py --benchmark-only
"""

import asyncio

import bundlebase
import pytest


@pytest.fixture(scope="module")
def event_loop():
    """Create event loop for async operations."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


@pytest.fixture(scope="module")
def bundle_with_data(event_loop, data_path_1k):
    """Create a bundle with test data for conversion benchmarks."""

    async def setup():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        return c

    return event_loop.run_until_complete(setup())


def test_to_pandas(benchmark, bundle_with_data, event_loop):
    """Benchmark converting to pandas DataFrame."""

    async def convert():
        return await bundlebase.to_pandas(bundle_with_data)

    def run():
        return event_loop.run_until_complete(convert())

    result = benchmark(run)
    assert len(result) > 0


def test_to_polars(benchmark, bundle_with_data, event_loop):
    """Benchmark converting to polars DataFrame."""
    polars = pytest.importorskip("polars")

    async def convert():
        return await bundlebase.to_polars(bundle_with_data)

    def run():
        return event_loop.run_until_complete(convert())

    result = benchmark(run)
    assert len(result) > 0


def test_to_pandas_with_filter(benchmark, event_loop, data_path_1k):
    """Benchmark converting filtered data to pandas."""

    async def filter_and_convert():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        c = await c.filter("SELECT * FROM bundle WHERE amount > 5000")
        return await bundlebase.to_pandas(c)

    def run():
        return event_loop.run_until_complete(filter_and_convert())

    result = benchmark(run)
    assert len(result) > 0


def test_to_pandas_with_projection(benchmark, event_loop, data_path_1k):
    """Benchmark converting projected data to pandas."""

    async def project_and_convert():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        c = await c.filter("SELECT id, name, amount FROM bundle")
        return await bundlebase.to_pandas(c)

    def run():
        return event_loop.run_until_complete(project_and_convert())

    result = benchmark(run)
    assert len(result) > 0
    assert set(result.columns) == {'id', 'name', 'amount'}


def test_stream_batches(benchmark, bundle_with_data, event_loop):
    """Benchmark streaming batches."""

    async def stream_all():
        total_rows = 0
        async for batch in bundlebase.stream_batches(bundle_with_data):
            total_rows += batch.num_rows
        return total_rows

    def run():
        return event_loop.run_until_complete(stream_all())

    result = benchmark(run)
    assert result > 0


def test_stream_batches_with_filter(benchmark, event_loop, data_path_1k):
    """Benchmark streaming filtered data."""

    async def stream_filtered():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        c = await c.filter("SELECT * FROM bundle WHERE amount > 5000")

        total_rows = 0
        async for batch in bundlebase.stream_batches(c):
            total_rows += batch.num_rows
        return total_rows

    def run():
        return event_loop.run_until_complete(stream_filtered())

    result = benchmark(run)
    assert result >= 0


def test_as_pyarrow(benchmark, bundle_with_data, event_loop):
    """Benchmark converting to PyArrow Table.

    Note: This uses as_pyarrow() which may not be streaming.
    For large datasets, prefer stream_batches() instead.
    """

    async def convert():
        return await bundle_with_data.as_pyarrow()

    def run():
        return event_loop.run_until_complete(convert())

    result = benchmark(run)
    assert len(result) > 0
