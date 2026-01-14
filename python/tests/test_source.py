"""Tests for Python bindings of source definition and fetch functionality.

Note: The core source/fetch logic is tested in Rust E2E tests.
These tests verify that the Python bindings work correctly.
"""

import os
import shutil
import tempfile

import maturin_import_hook
import pytest

maturin_import_hook.install()

import bundlebase
from conftest import random_bundle


@pytest.mark.asyncio
async def test_create_source_binding():
    """Test that create_source Python binding works."""
    c = await bundlebase.create(random_bundle())
    c = await c.create_source("remote_dir", {"url": "file:///some/path/"})
    assert c is not None


@pytest.mark.asyncio
async def test_create_source_with_patterns_binding():
    """Test that create_source with patterns Python binding works."""
    c = await bundlebase.create(random_bundle())
    c = await c.create_source("remote_dir", {"url": "file:///data/", "patterns": "**/*.parquet,**/*.csv"})
    assert c is not None


@pytest.mark.asyncio
async def test_create_source_chaining():
    """Test that create_source works with operation chaining."""
    c = await (bundlebase.create(random_bundle())
               .set_name("Test Bundle")
               .create_source("remote_dir", {"url": "file:///data/", "patterns": "**/*.parquet"}))
    assert c is not None
    assert c.name == "Test Bundle"


@pytest.mark.asyncio
async def test_create_source_auto_fetch():
    """Test that create_source automatically fetches and attaches files."""
    with tempfile.TemporaryDirectory() as source_dir:
        # Copy test file to source directory
        src_path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
            "test_data", "userdata.parquet"
        )
        if os.path.exists(src_path):
            shutil.copy(src_path, os.path.join(source_dir, "userdata.parquet"))

            c = await bundlebase.create(random_bundle())
            source_url = f"file://{source_dir}/"
            c = await c.create_source("remote_dir", {"url": source_url, "patterns": "**/*.parquet"})

            # Data should be auto-attached
            assert await c.num_rows() == 1000


@pytest.mark.asyncio
async def test_fetch_returns_count():
    """Test that fetch returns the count of newly attached files."""
    with tempfile.TemporaryDirectory() as source_dir:
        c = await bundlebase.create(random_bundle())
        source_url = f"file://{source_dir}/"
        c = await c.create_source("remote_dir", {"url": source_url, "patterns": "**/*"})

        # Add a file after create_source
        src_path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
            "test_data", "userdata.parquet"
        )
        if os.path.exists(src_path):
            shutil.copy(src_path, os.path.join(source_dir, "userdata.parquet"))

            # fetch should return 1 (default pack is "base")
            count = await c.fetch()
            assert count == 1
