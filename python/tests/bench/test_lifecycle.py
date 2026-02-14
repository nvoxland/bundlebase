"""Bundle lifecycle benchmarks.

Benchmarks for create, open, attach, and commit operations.

Run with: poetry run pytest python/tests/bench/test_lifecycle.py --benchmark-only
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


def test_create_empty_bundle(benchmark, event_loop):
    """Benchmark creating an empty bundle."""

    async def create_bundle():
        return await bundlebase.create(bundlebase.random_memory_url())

    def run():
        return event_loop.run_until_complete(create_bundle())

    benchmark(run)


def test_create_and_attach(benchmark, event_loop, data_path_1k):
    """Benchmark creating a bundle and attaching data."""

    async def create_and_attach():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        return c

    def run():
        return event_loop.run_until_complete(create_and_attach())

    benchmark(run)


def test_create_and_commit(benchmark, event_loop, data_path_1k):
    """Benchmark creating a bundle with data and committing."""

    async def create_and_commit():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        c = await c.commit("Benchmark commit")
        return c

    def run():
        return event_loop.run_until_complete(create_and_commit())

    benchmark(run)


def test_attach_with_operations(benchmark, event_loop, data_path_1k):
    """Benchmark attaching data with additional operations."""

    async def attach_with_ops():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        c = await c.drop_column("region")
        c = await c.rename_column("category", "cat")
        return c

    def run():
        return event_loop.run_until_complete(attach_with_ops())

    benchmark(run)


def test_filter_operation(benchmark, event_loop, data_path_1k):
    """Benchmark applying a filter operation."""

    async def apply_filter():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        c = await c.filter("SELECT * FROM bundle WHERE amount > 5000")
        return c

    def run():
        return event_loop.run_until_complete(apply_filter())

    benchmark(run)


def test_filter_with_projection(benchmark, event_loop, data_path_1k):
    """Benchmark applying a filter with column projection."""

    async def run_filter():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        c = await c.filter("SELECT id, name, amount FROM bundle WHERE amount > 5000")
        return c

    def run():
        return event_loop.run_until_complete(run_filter())

    benchmark(run)


def test_num_rows(benchmark, event_loop, data_path_1k):
    """Benchmark counting rows (forces query execution)."""

    # Setup: create bundle once
    async def setup():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_1k)
        return c

    bundle = event_loop.run_until_complete(setup())

    async def count_rows():
        return await bundle.num_rows()

    def run():
        return event_loop.run_until_complete(count_rows())

    benchmark(run)
