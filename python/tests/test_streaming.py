"""Tests for streaming data access.

This module tests the streaming API which enables processing large datasets
without loading everything into memory.
"""
import bundlebase
import pandas as pd
import pyarrow as pa
import pytest


@pytest.mark.asyncio
async def test_stream_batches_basic():
    """Test basic streaming functionality."""
    # Create a bundle with test data
    c = await bundlebase.create()
    c = await c.attach(bundlebase.test_datafile("userdata.parquet"))

    # Stream batches and count rows
    total_rows = 0
    batch_count = 0

    async for batch in bundlebase.stream_batches(c):
        assert isinstance(batch, pa.RecordBatch)
        assert batch.num_rows > 0
        total_rows += batch.num_rows
        batch_count += 1

    # Verify we got data
    assert total_rows > 0
    assert batch_count > 0

    assert total_rows == await c.num_rows()


@pytest.mark.asyncio
async def test_to_pandas_uses_streaming():
    """Verify to_pandas() now uses streaming internally."""
    c = await bundlebase.create()
    c = await c.attach(bundlebase.test_datafile("userdata.parquet"))

    # This should use streaming internally
    df = await bundlebase.to_pandas(c)

    assert isinstance(df, pd.DataFrame)
    assert len(df) > 0
    assert len(df) == await c.num_rows()


@pytest.mark.asyncio
async def test_stream_with_operations():
    """Test streaming with operations applied."""
    c = await bundlebase.create()
    c = await c.attach(bundlebase.test_datafile("userdata.parquet"))
    c = await c.filter("gender = 'Male'")

    # Stream the filtered data
    batches = []
    async for batch in bundlebase.stream_batches(c):
        batches.append(batch)

    assert len(batches) > 0

    # Verify filtered data matches expected
    df = await bundlebase.to_pandas(c)
    assert len(df) > 0
    # All rows should have gender = 'Male'
    assert (df['gender'] == 'Male').all()


@pytest.mark.asyncio
async def test_stream_batches_empty_bundle():
    """Test streaming with no data."""
    c = await bundlebase.create()
    c = await c.attach(bundlebase.test_datafile("userdata.parquet"))
    # Filter that matches nothing
    c = await c.filter("1 = 0")

    batch_count = 0
    async for batch in bundlebase.stream_batches(c):
        batch_count += 1

    # Empty result should yield no batches
    assert batch_count == 0


@pytest.mark.asyncio
async def test_stream_batches_with_extending_bundle():
    """Test streaming works with PyBundleBuilder."""
    c = await bundlebase.create()
    c = c.attach(bundlebase.test_datafile("userdata.parquet"))

    # BundleBuilder should also support streaming
    total_rows = 0
    async for batch in bundlebase.stream_batches(c):
        total_rows += batch.num_rows

    assert total_rows > 0


@pytest.mark.asyncio
async def test_as_pyarrow_stream_direct_access():
    """Test direct access to the PyRecordBatchStream object."""
    c = await bundlebase.create()
    c = await c.attach(bundlebase.test_datafile("userdata.parquet"))

    # Get the stream object directly
    stream = await c.as_pyarrow_stream()

    # Verify stream has expected methods
    assert hasattr(stream, 'next_batch')
    assert hasattr(stream, 'schema')

    # Read batches using direct API
    batch = await stream.next_batch()
    assert batch is not None
    assert isinstance(batch, pa.RecordBatch)

    # Get schema
    schema = stream.schema
    assert schema is not None
    assert len(schema.fields) > 0


@pytest.mark.asyncio
async def test_stream_consistency_with_collect():
    """Verify streaming produces same results as collecting."""
    c = await bundlebase.create()
    c = await c.attach(bundlebase.test_datafile("userdata.parquet"))

    # Get data via streaming
    batches_streamed = []
    async for batch in bundlebase.stream_batches(c):
        batches_streamed.append(batch)

    # Get data via collection
    arrow_data = await c.as_pyarrow()

    # Compare row counts
    streamed_rows = sum(b.num_rows for b in batches_streamed)
    collected_rows = sum(b.num_rows for b in arrow_data)

    assert streamed_rows == collected_rows
