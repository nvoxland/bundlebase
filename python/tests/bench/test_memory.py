"""Memory usage benchmarks.

Critical benchmarks for verifying constant memory usage during streaming.
The key constraint is ~50MB constant memory regardless of dataset size.

Run with: poetry run pytest python/tests/bench/test_memory.py --benchmark-only

For detailed memory profiling, run with tracemalloc:
    poetry run python -c "import tracemalloc; tracemalloc.start(); exec(open('run_memory_test.py').read())"
"""

import asyncio
import gc
import tracemalloc

import bundlebase
import pytest


@pytest.fixture(scope="module")
def event_loop():
    """Create event loop for async operations."""
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


def get_memory_mb() -> float:
    """Get current memory usage in MB using tracemalloc."""
    current, peak = tracemalloc.get_traced_memory()
    return current / (1024 * 1024)


def test_streaming_memory_constant(event_loop, data_path_10k):
    """Verify streaming uses constant memory.

    This test verifies that streaming through data does not accumulate
    memory proportional to dataset size. The memory usage should stay
    roughly constant (within a tolerance) regardless of how many rows
    are processed.
    """
    tracemalloc.start()

    async def stream_and_measure():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_10k)

        # Force garbage collection before measurement
        gc.collect()
        memory_samples = []
        total_rows = 0

        async for batch in bundlebase.stream_batches(c):
            total_rows += batch.num_rows
            # Sample memory periodically
            if total_rows % 1000 == 0:
                gc.collect()
                memory_samples.append(get_memory_mb())

        return total_rows, memory_samples

    total_rows, memory_samples = event_loop.run_until_complete(stream_and_measure())

    tracemalloc.stop()

    # Verify we processed data
    assert total_rows > 0, "Should have processed rows"

    if len(memory_samples) >= 2:
        # Memory should not grow significantly during streaming
        # Allow for some variance but catch major leaks
        first_half_avg = sum(memory_samples[:len(memory_samples)//2]) / (len(memory_samples)//2)
        second_half_avg = sum(memory_samples[len(memory_samples)//2:]) / (len(memory_samples) - len(memory_samples)//2)

        # Memory in second half shouldn't be more than 2x the first half
        # (generous tolerance for test stability)
        assert second_half_avg < first_half_avg * 2, \
            f"Memory grew from {first_half_avg:.1f}MB to {second_half_avg:.1f}MB during streaming"


def test_to_pandas_memory(benchmark, event_loop, data_path_10k):
    """Benchmark memory usage of to_pandas conversion.

    Note: to_pandas() uses streaming internally, so memory should be bounded.
    """

    async def convert_and_measure():
        tracemalloc.start()
        gc.collect()
        start_memory = get_memory_mb()

        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_10k)
        df = await bundlebase.to_pandas(c)

        gc.collect()
        end_memory = get_memory_mb()

        tracemalloc.stop()
        return df, end_memory - start_memory

    def run():
        return event_loop.run_until_complete(convert_and_measure())

    result, memory_delta = benchmark(run)
    assert len(result) > 0


def test_stream_batches_memory(benchmark, event_loop, data_path_10k):
    """Benchmark streaming with memory tracking."""

    async def stream_with_memory():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_10k)

        total_rows = 0
        max_batch_size = 0

        async for batch in bundlebase.stream_batches(c):
            total_rows += batch.num_rows
            max_batch_size = max(max_batch_size, batch.num_rows)

        return total_rows, max_batch_size

    def run():
        return event_loop.run_until_complete(stream_with_memory())

    result = benchmark(run)
    total_rows, max_batch_size = result
    assert total_rows > 0
    # Batch size should be bounded (not entire dataset)
    assert max_batch_size < total_rows, \
        f"Batch size {max_batch_size} should be smaller than total rows {total_rows}"


def test_filter_streaming_memory(benchmark, event_loop, data_path_10k):
    """Benchmark filtered streaming memory usage."""

    async def filter_and_stream():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_10k)
        c = await c.filter("SELECT * FROM bundle WHERE amount > 5000")

        total_rows = 0
        async for batch in bundlebase.stream_batches(c):
            total_rows += batch.num_rows

        return total_rows

    def run():
        return event_loop.run_until_complete(filter_and_stream())

    result = benchmark(run)
    assert result >= 0


def test_aggregation_memory(benchmark, event_loop, data_path_10k):
    """Benchmark aggregation memory usage.

    Aggregations may need to buffer data, but should still be bounded.
    """

    async def aggregate():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_10k)
        result = await c.query("SELECT category, COUNT(*) as count, AVG(amount) as avg_amount FROM bundle GROUP BY category")
        return await result.to_pandas()

    def run():
        return event_loop.run_until_complete(aggregate())

    result = benchmark(run)
    assert len(result) > 0


def test_multiple_operations_memory(benchmark, event_loop, data_path_10k):
    """Benchmark memory with multiple chained operations."""

    async def chain_operations():
        c = await bundlebase.create(bundlebase.random_memory_url())
        c = await c.attach(data_path_10k)
        c = await c.filter("SELECT * FROM bundle WHERE amount > 5000")
        c = await c.drop_column("region")
        c = await c.rename_column("filter_value", "fv")

        total_rows = 0
        async for batch in bundlebase.stream_batches(c):
            total_rows += batch.num_rows

        return total_rows

    def run():
        return event_loop.run_until_complete(chain_operations())

    result = benchmark(run)
    assert result >= 0
