"""Smoke coverage for absolute attach paths through both Python interfaces."""

import pytest

import bundlebase
import bundlebase.sync as sync


@pytest.mark.asyncio
async def test_async_attach_absolute_path(tmp_path):
    data = tmp_path / "sales 100%.csv"
    data.write_text("id,amount\n1,10\n")
    builder = await bundlebase.create(str(tmp_path / "bundle")).attach(str(data))
    assert await builder.num_rows() == 1


def test_sync_attach_absolute_path(tmp_path):
    data = tmp_path / "sales 100%.csv"
    data.write_text("id,amount\n1,10\n")
    builder = sync.create(str(tmp_path / "bundle")).attach(str(data))
    assert builder.num_rows() == 1
