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
async def test_create_kaggle_source_invalid_dataset():
    """Test that kaggle source function validates dataset format."""
    c = await bundlebase.create(random_bundle())
    with pytest.raises(ValueError, match="Invalid dataset format"):
        await c.create_source("kaggle", {"dataset": "invalid-no-slash"})


@pytest.mark.asyncio
async def test_create_kaggle_source_missing_dataset():
    """Test that kaggle source function requires dataset argument."""
    c = await bundlebase.create(random_bundle())
    with pytest.raises(ValueError, match="requires a 'dataset' argument"):
        await c.create_source("kaggle", {})


@pytest.mark.asyncio
async def test_create_kaggle_source_credentials():
    """Test that kaggle source passes validation and reaches credential check."""
    c = await bundlebase.create(random_bundle())
    # With valid args but no ~/.kaggle/kaggle.json, create_source should fail
    # with a credential error (proving validation passed and discover() was reached)
    if not os.path.exists(os.path.expanduser("~/.kaggle/kaggle.json")):
        with pytest.raises(ValueError, match="kaggle.json"):
            await c.create_source("kaggle", {"dataset": "zillow/zecon"})


@pytest.mark.asyncio
async def test_fetch_returns_results():
    """Test that fetch returns FetchResults with details about attached files."""
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

            # fetch should return a list of FetchResults
            results = await c.fetch()
            assert len(results) == 1  # One source
            result = results[0]
            assert result.source_function == "remote_dir"
            assert len(result.added) == 1  # One file added
            assert result.added[0].source_location == "userdata.parquet"
            assert result.pack == "base"
            assert len(result.replaced) == 0
            assert len(result.removed) == 0
            assert result.total_count() == 1
            assert not result.is_empty()
