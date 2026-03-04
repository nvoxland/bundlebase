"""Tests for Python bindings of source definition and fetch functionality.

Note: The core source/fetch logic is tested in Rust E2E tests.
These tests verify that the Python bindings work correctly.
"""

import os
import shutil
import tempfile

import maturin_import_hook
import pyarrow as pa
import pytest

maturin_import_hook.install()

import bundlebase
from bundlebase_sdk import SourceFunction, Location, StableUrl
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
            results = await c.fetch("base", "add")
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


ALLOW_EXTERNAL_CODE_CONFIG = {"system": {"allow_external_code": "true"}}


# ---- Plugin source (create_source_plugin) tests ----

class SimplePluginSource(SourceFunction):
    """A minimal plugin source for testing."""

    def discover(self, attached_locations, **kwargs):
        return [
            Location("data1.parquet", must_copy=True, format="parquet", version="v1"),
            Location("data2.parquet", must_copy=True, format="parquet", version="v1"),
        ]

    def data(self, location, **kwargs):
        if location.location == "data1.parquet":
            return pa.table({"id": [1, 2, 3], "name": ["a", "b", "c"]})
        elif location.location == "data2.parquet":
            return pa.table({"id": [4, 5], "name": ["d", "e"]})
        return None


class StableUrlPluginSource(SourceFunction):
    """Plugin source with stable_url support."""

    def discover(self, attached_locations, **kwargs):
        return [Location("cached.parquet", version="v1")]

    def data(self, location, **kwargs):
        return pa.table({"x": [1]})

    def stable_url(self, location, **kwargs):
        return StableUrl("https://example.com/cached.parquet")


class KwargsPluginSource(SourceFunction):
    """Plugin source that echoes extra kwargs back."""

    def discover(self, attached_locations, **kwargs):
        name = kwargs.get("custom_key", "missing")
        return [Location(name, version="v1")]

    def data(self, location, **kwargs):
        return pa.table({"val": [kwargs.get("custom_key", "none")]})


@pytest.mark.asyncio
async def test_create_source_plugin_binding():
    """Test that create_source_plugin Python binding works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.create_source_plugin(SimplePluginSource())
    assert c is not None


@pytest.mark.asyncio
async def test_create_source_plugin_fetch():
    """Test that plugin source discovers and attaches data via fetch."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.create_source_plugin(SimplePluginSource())

    # create_source_plugin auto-fetches, so data is already present.
    # A subsequent fetch should find no new data.
    results = await c.fetch("base", "add")
    assert len(results) == 1
    result = results[0]
    assert result.source_function == "plugin"
    assert result.total_count() == 0

    # But the data should be queryable
    rows = await c.num_rows()
    assert rows == 5  # 3 from data1 + 2 from data2


@pytest.mark.asyncio
async def test_create_source_plugin_data():
    """Test that data from plugin source is queryable."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.create_source_plugin(SimplePluginSource())
    await c.fetch("base", "add")

    rows = await c.num_rows()
    assert rows == 5  # 3 from data1 + 2 from data2


@pytest.mark.asyncio
async def test_create_source_plugin_second_fetch_no_changes():
    """Test that second fetch detects no new data."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.create_source_plugin(SimplePluginSource())

    # create_source_plugin auto-fetches, so first explicit fetch finds nothing new
    results1 = await c.fetch("base", "add")
    assert results1[0].total_count() == 0
    assert results1[0].is_empty()


@pytest.mark.asyncio
async def test_create_source_plugin_with_kwargs():
    """Test that extra kwargs are passed to discover and data."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.create_source_plugin(KwargsPluginSource(), custom_key="hello")

    # create_source_plugin auto-fetches, so data is already present
    rows = await c.num_rows()
    assert rows == 1


@pytest.mark.asyncio
async def test_create_source_plugin_with_stable_url():
    """Test that plugin source with stable_url works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.create_source_plugin(StableUrlPluginSource())

    # create_source_plugin auto-fetches, so data is already present
    rows = await c.num_rows()
    assert rows == 1


# ---- External code gate tests ----


@pytest.mark.asyncio
async def test_plugin_source_blocked_without_allow_external_code():
    """Test that plugin source fails when allow_external_code is not set."""
    c = await bundlebase.create(random_bundle())

    with pytest.raises(ValueError, match="External code execution is disabled"):
        await c.create_source_plugin(SimplePluginSource())


@pytest.mark.asyncio
async def test_plugin_source_allowed_with_config():
    """Test that plugin source works when allow_external_code=true."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.create_source_plugin(SimplePluginSource())

    # create_source_plugin auto-fetches, so data should already be present
    rows = await c.num_rows()
    assert rows == 5  # 3 from data1 + 2 from data2
