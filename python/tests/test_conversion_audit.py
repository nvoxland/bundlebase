"""Regression tests for July 2026 audit findings in conversions/streaming (bb-cko)."""

import pytest

import bundlebase
import bundlebase.sync as bb
from conftest import datafile, random_bundle


def test_empty_result_to_pandas_returns_empty_df():
    """bb-cko: a legitimately empty result must return an empty DataFrame, not raise."""
    c = bb.create(random_bundle())
    c.attach(datafile("userdata.parquet"))
    c.filter("SELECT * FROM bundle WHERE 1 = 0")
    df = c.to_pandas()
    assert len(df) == 0


def test_empty_result_to_polars_returns_empty_df():
    c = bb.create(random_bundle())
    c.attach(datafile("userdata.parquet"))
    c.filter("SELECT * FROM bundle WHERE 1 = 0")
    df = c.to_polars()
    assert len(df) == 0


@pytest.mark.asyncio
async def test_stream_is_empty_peeks_and_terminates():
    """bb-cko: is_empty() must reflect real exhaustion (was hardcoded False)."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))
    stream = await c.as_pyarrow_stream()

    # Not empty before consuming anything.
    assert (await stream.is_empty()) is False

    # Draining via is_empty() must terminate and not drop the peeked batch.
    n_batches = 0
    while not await stream.is_empty():
        batch = await stream.next_batch()
        assert batch is not None
        n_batches += 1

    assert n_batches >= 1
    assert (await stream.is_empty()) is True


def test_unknown_test_datafile_raises_value_error():
    """bb-cko.8: unknown datafile name must raise ValueError with the available
    names, not a PanicException from the Rust panic."""
    with pytest.raises(ValueError, match="No test datafile named 'nonexistent'"):
        bundlebase.test_datafile("nonexistent")
    with pytest.raises(ValueError, match="Available: "):
        bundlebase.test_datafile("nonexistent")


class _NoCollectProxy:
    """Wraps a bundle; any touch of as_pyarrow (the collect() path) fails."""

    def __init__(self, inner):
        self._inner = inner

    def __getattr__(self, name):
        if name == "as_pyarrow":
            raise AssertionError("collect() path used — must stream (bb-cko.3)")
        return getattr(self._inner, name)


@pytest.mark.asyncio
async def test_to_dict_streams_not_collects():
    """bb-cko.3: to_dict must route through stream_batches, not as_pyarrow."""
    from bundlebase import conversion

    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))
    data = await conversion.to_dict(_NoCollectProxy(c))
    assert len(data) > 0
    assert all(len(col) > 0 for col in data.values())


@pytest.mark.asyncio
async def test_to_numpy_streams_not_collects():
    """bb-cko.3: to_numpy must route through stream_batches, not as_pyarrow."""
    from bundlebase import conversion

    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))
    arrays = await conversion.to_numpy(_NoCollectProxy(c))
    assert len(arrays) > 0
    assert all(len(arr) > 0 for arr in arrays.values())
