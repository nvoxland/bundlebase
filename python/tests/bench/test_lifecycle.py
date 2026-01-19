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


def test_create_and_attach(benchmark, event_loop):
    """Benchmark creating a bundle and attaching data."""

    async def create_and_attach():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(bundlebase.test_datafile("userdata.parquet"))
        return c

    def run():
        return event_loop.run_until_complete(create_and_attach())

    benchmark(run)


def test_create_and_commit(benchmark, event_loop):
    """Benchmark creating a bundle with data and committing."""

    async def create_and_commit():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(bundlebase.test_datafile("userdata.parquet"))
        c = await c.commit("Benchmark commit")
        return c

    def run():
        return event_loop.run_until_complete(create_and_commit())

    benchmark(run)


def test_attach_with_operations(benchmark, event_loop):
    """Benchmark attaching data with additional operations."""

    async def attach_with_ops():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(bundlebase.test_datafile("userdata.parquet"))
        c = await c.drop_column("title")
        c = await c.rename_column("first_name", "name")
        return c

    def run():
        return event_loop.run_until_complete(attach_with_ops())

    benchmark(run)


def test_filter_operation(benchmark, event_loop):
    """Benchmark applying a filter operation."""

    async def apply_filter():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(bundlebase.test_datafile("userdata.parquet"))
        c = await c.filter("salary > 50000")
        return c

    def run():
        return event_loop.run_until_complete(apply_filter())

    benchmark(run)


def test_select_operation(benchmark, event_loop):
    """Benchmark applying a select/query operation."""

    async def run_select():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(bundlebase.test_datafile("userdata.parquet"))
        c = await c.select("first_name, last_name, salary WHERE salary > 50000")
        return c

    def run():
        return event_loop.run_until_complete(run_select())

    benchmark(run)


def test_num_rows(benchmark, event_loop):
    """Benchmark counting rows (forces query execution)."""

    # Setup: create bundle once
    async def setup():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(bundlebase.test_datafile("userdata.parquet"))
        return c

    bundle = event_loop.run_until_complete(setup())

    async def count_rows():
        return await bundle.num_rows()

    def run():
        return event_loop.run_until_complete(count_rows())

    benchmark(run)
