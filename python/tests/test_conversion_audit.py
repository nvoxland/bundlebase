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
