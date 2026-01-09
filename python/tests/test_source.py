"""Tests for Python bindings of source definition and refresh functionality.

Note: The core source/refresh logic is tested in Rust E2E tests.
These tests verify that the Python bindings work correctly.
"""

import os
import shutil
import tempfile

import maturin_import_hook
import pytest

maturin_import_hook.install()

import bundlebase
from conftest import datafile, random_bundle


@pytest.mark.asyncio
async def test_define_source_binding():
    """Test that define_source Python binding works."""
    with tempfile.TemporaryDirectory() as temp_dir:
        c = await bundlebase.create(temp_dir)
        c = await c.define_source("file:///some/path/", function="data_directory")
        assert c is not None


@pytest.mark.asyncio
async def test_define_source_with_patterns_binding():
    """Test that define_source with patterns Python binding works."""
    with tempfile.TemporaryDirectory() as temp_dir:
        c = await bundlebase.create(temp_dir)
        c = await c.define_source("file:///data/", patterns=["**/*.parquet", "**/*.csv"], function="data_directory")
        assert c is not None


@pytest.mark.asyncio
async def test_define_source_chaining():
    """Test that define_source works with operation chaining."""
    with tempfile.TemporaryDirectory() as temp_dir:
        c = await (bundlebase.create(temp_dir)
                   .set_name("Test Bundle")
                   .define_source("file:///data/", patterns=["**/*.parquet"], function="data_directory"))
        assert c is not None
        assert c.name == "Test Bundle"


@pytest.mark.asyncio
async def test_refresh_binding():
    """Test that refresh Python binding works and returns int."""
    with tempfile.TemporaryDirectory() as bundle_dir:
        with tempfile.TemporaryDirectory() as source_dir:
            c = await bundlebase.create(bundle_dir)
            source_url = f"file://{source_dir}/"
            c = await c.define_source(source_url, patterns=["**/*.parquet"], function="data_directory")

            # refresh should return an integer
            count = await c.refresh()
            assert isinstance(count, int)
            assert count == 0  # Empty source directory


@pytest.mark.asyncio
async def test_check_refresh_binding():
    """Test that check_refresh Python binding works and returns list of tuples."""
    with tempfile.TemporaryDirectory() as bundle_dir:
        with tempfile.TemporaryDirectory() as source_dir:
            c = await bundlebase.create(bundle_dir)
            source_url = f"file://{source_dir}/"
            c = await c.define_source(source_url, patterns=["**/*.parquet"], function="data_directory")

            # check_refresh should return a list
            pending = await c.check_refresh()
            assert isinstance(pending, list)
            assert len(pending) == 0  # Empty source directory


@pytest.mark.asyncio
async def test_define_source_auto_refresh():
    """Test that define_source automatically refreshes and attaches files."""
    with tempfile.TemporaryDirectory() as bundle_dir:
        with tempfile.TemporaryDirectory() as source_dir:
            # Copy test file to source directory
            src_path = os.path.join(
                os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
                "test_data", "userdata.parquet"
            )
            if os.path.exists(src_path):
                shutil.copy(src_path, os.path.join(source_dir, "userdata.parquet"))

                c = await bundlebase.create(bundle_dir)
                source_url = f"file://{source_dir}/"
                c = await c.define_source(source_url, patterns=["**/*.parquet"], function="data_directory")

                # Data should be auto-attached
                assert await c.num_rows() == 1000


@pytest.mark.asyncio
async def test_refresh_returns_count():
    """Test that refresh returns the count of newly attached files."""
    with tempfile.TemporaryDirectory() as bundle_dir:
        with tempfile.TemporaryDirectory() as source_dir:
            c = await bundlebase.create(bundle_dir)
            source_url = f"file://{source_dir}/"
            c = await c.define_source(source_url, patterns=["**/*"], function="data_directory")

            # Add a file after define_source
            src_path = os.path.join(
                os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
                "test_data", "userdata.parquet"
            )
            if os.path.exists(src_path):
                shutil.copy(src_path, os.path.join(source_dir, "userdata.parquet"))

                # Refresh should return 1
                count = await c.refresh()
                assert count == 1


@pytest.mark.asyncio
async def test_check_refresh_returns_pending():
    """Test that check_refresh returns pending files as (source_id, url) tuples."""
    with tempfile.TemporaryDirectory() as bundle_dir:
        with tempfile.TemporaryDirectory() as source_dir:
            c = await bundlebase.create(bundle_dir)
            source_url = f"file://{source_dir}/"
            c = await c.define_source(source_url, patterns=["**/*"], function="data_directory")

            # Add a file after define_source
            src_path = os.path.join(
                os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
                "test_data", "userdata.parquet"
            )
            if os.path.exists(src_path):
                shutil.copy(src_path, os.path.join(source_dir, "userdata.parquet"))

                # check_refresh should return tuple with (source_id, url)
                pending = await c.check_refresh()
                assert len(pending) == 1
                source_id, url = pending[0]
                assert isinstance(source_id, str)
                assert isinstance(url, str)
                assert "userdata.parquet" in url
