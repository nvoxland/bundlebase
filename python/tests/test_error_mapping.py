"""Tests for Python error mapping (BUG 3).

Opening a nonexistent bundle should raise FileNotFoundError (a subclass of
OSError), which is a more accurate UX than ValueError for missing data.
"""

import tempfile
import os

import pytest

import bundlebase


@pytest.mark.asyncio
async def test_open_nonexistent_bundle_raises_filenotfound():
    with tempfile.TemporaryDirectory() as tmpdir:
        missing = os.path.join(tmpdir, "does_not_exist_bundle")
        with pytest.raises(FileNotFoundError):
            await bundlebase.open(missing)


@pytest.mark.asyncio
async def test_open_nonexistent_bundle_message_preserved():
    with tempfile.TemporaryDirectory() as tmpdir:
        missing = os.path.join(tmpdir, "does_not_exist_bundle")
        with pytest.raises(FileNotFoundError) as exc_info:
            await bundlebase.open(missing)
        assert "No bundle found" in str(exc_info.value)
