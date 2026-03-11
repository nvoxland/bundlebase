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
from bundlebase_sdk import Connector, Location, StableUrl
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
    """Test that kaggle connector validates dataset format."""
    c = await bundlebase.create(random_bundle())
    with pytest.raises(ValueError, match="Invalid dataset format"):
        await c.create_source("kaggle", {"dataset": "invalid-no-slash"})


@pytest.mark.asyncio
async def test_create_kaggle_source_missing_dataset():
    """Test that kaggle connector requires dataset argument."""
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
            assert result.connector == "remote_dir"
            assert len(result.added) == 1  # One file added
            assert result.added[0].source_location == "userdata.parquet"
            assert result.pack == "base"
            assert len(result.replaced) == 0
            assert len(result.removed) == 0
            assert result.total_count() == 1
            assert not result.is_empty()


ALLOW_EXTERNAL_CODE_CONFIG = {"system": {"allow_external_code": "true"}}


# ---- Define source / set source logic / native source tests ----

class SimpleNativeSource(Connector):
    """A minimal native source for testing."""

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


class StableUrlNativeSource(Connector):
    """Native source with stable_url support."""

    def discover(self, attached_locations, **kwargs):
        return [Location("cached.parquet", version="v1")]

    def data(self, location, **kwargs):
        return pa.table({"x": [1]})

    def stable_url(self, location, **kwargs):
        return StableUrl("https://example.com/cached.parquet")


class KwargsNativeSource(Connector):
    """Native source that echoes extra kwargs back."""

    def discover(self, attached_locations, **kwargs):
        name = kwargs.get("custom_key", "missing")
        return [Location(name, version="v1")]

    def data(self, location, **kwargs):
        return pa.table({"val": [kwargs.get("custom_key", "none")]})


# Helper to set up a native source via import_temp_connector + create_source
async def _setup_native_source(c, source_class, source_name="test.native_source", **kwargs):
    """Define temporary connector and create a native source."""
    module = source_class.__module__
    qualname = source_class.__qualname__

    c = await c.import_temp_connector(source_name, "python", f"{module}:{qualname}")
    args = {k: str(v) for k, v in kwargs.items()}
    c = await c.create_source(source_name, args)
    return c


@pytest.mark.asyncio
async def test_import_connector_binding():
    """Test that import_connector Python binding works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test")
    assert c is not None


@pytest.mark.asyncio
async def test_import_temp_connector_binding():
    """Test that import_temp_connector Python binding works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_temp_connector("test.my_source", "python", "mod:Class")
    assert c is not None


@pytest.mark.asyncio
async def test_import_temp_connector_builder_version_uncommitted_temp():
    """Test that import_temp_connector on builder with changes sets version to UNCOMMITTED+TEMP."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    assert c.version != "UNCOMMITTED+TEMP"
    # Add a persistent change so version becomes UNCOMMITTED
    c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test")
    assert c.version == "UNCOMMITTED"
    # Now add a temporary connector to get UNCOMMITTED+TEMP
    c = await c.import_temp_connector("test.temp_source", "python", "mod:Class")
    assert c.version == "UNCOMMITTED+TEMP"


@pytest.mark.asyncio
async def test_import_temp_connector_bundle_version_temp():
    """Test that import_temp_connector on read-only bundle sets version to TEMP."""
    with tempfile.TemporaryDirectory() as path:
        c = await bundlebase.create(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test")
        await c.commit("Initial commit")

        bundle = await bundlebase.open(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        assert bundle.version != "TEMP"
        await bundle.import_temp_connector("test.my_source", "python", "mod:Class")
        assert bundle.version == "TEMP"


@pytest.mark.asyncio
async def test_import_connector_rejects_python_calls():
    """Test that import_connector rejects python runner."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="python runner cannot be bundled"):
        await c.import_connector("test.my_source", "python", "mod:Class")


@pytest.mark.asyncio
async def test_import_connector_success_with_ipc():
    """Test that import_connector succeeds with ipc runner."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test")
    assert c is not None


@pytest.mark.asyncio
async def test_import_connector_and_set_logic_native_python():
    """Test import_connector, set logic (native python:...), create, verify data flows."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await _setup_native_source(c, SimpleNativeSource)

    rows = await c.num_rows()
    assert rows == 5  # 3 from data1 + 2 from data2


@pytest.mark.asyncio
async def test_native_source_fetch():
    """Test that native source discovers and attaches data via fetch."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await _setup_native_source(c, SimpleNativeSource)

    # create_source auto-fetches, so data is already present.
    # A subsequent fetch should find no new data.
    results = await c.fetch("base", "add")
    assert len(results) == 1
    result = results[0]
    assert result.connector == "test.native_source"
    assert result.total_count() == 0

    # But the data should be queryable
    rows = await c.num_rows()
    assert rows == 5  # 3 from data1 + 2 from data2


@pytest.mark.asyncio
async def test_native_source_data():
    """Test that data from native source is queryable."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await _setup_native_source(c, SimpleNativeSource)

    rows = await c.num_rows()
    assert rows == 5  # 3 from data1 + 2 from data2


@pytest.mark.asyncio
async def test_native_source_second_fetch_no_changes():
    """Test that second fetch detects no new data."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await _setup_native_source(c, SimpleNativeSource)

    # create_source auto-fetches, so first explicit fetch finds nothing new
    results1 = await c.fetch("base", "add")
    assert results1[0].total_count() == 0
    assert results1[0].is_empty()


@pytest.mark.asyncio
async def test_native_source_with_kwargs():
    """Test that extra kwargs are passed to discover and data."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await _setup_native_source(c, KwargsNativeSource, custom_key="hello")

    rows = await c.num_rows()
    assert rows == 1


@pytest.mark.asyncio
async def test_native_source_with_stable_url():
    """Test that native source with stable_url works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await _setup_native_source(c, StableUrlNativeSource)

    rows = await c.num_rows()
    assert rows == 1


# ---- Round-trip persistence tests ----


@pytest.mark.asyncio
async def test_import_connector_survives_commit_reopen():
    """Test that import_connector persists through commit and reopen."""
    with tempfile.TemporaryDirectory() as path:
        c = await bundlebase.create(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test")
        await c.commit("Add source definition")

        # Reopen and verify the source definition survived
        bundle = await bundlebase.open(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        builder = await bundle.extend()
        # Should not raise "not defined" — the source should exist
        c = await builder.drop_connector("test.my_source")
        assert c is not None


# ---- External code gate tests ----


@pytest.mark.asyncio
async def test_native_source_blocked_without_allow_external_code():
    """Test that native source fails when allow_external_code is not set."""
    c = await bundlebase.create(random_bundle())
    c = await c.import_temp_connector("test.blocked_source", "python", "mod:Class")

    with pytest.raises(ValueError, match="External code execution is disabled"):
        await c.create_source("test.blocked_source", {})


@pytest.mark.asyncio
async def test_native_source_allowed_with_config():
    """Test that native source works when allow_external_code=true."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await _setup_native_source(c, SimpleNativeSource)

    rows = await c.num_rows()
    assert rows == 5  # 3 from data1 + 2 from data2


# ---- Error cases ----


@pytest.mark.asyncio
async def test_create_source_undefined_name_fails():
    """Test that create_source with a dotted name that wasn't defined fails."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="not defined"):
        await c.create_source("test.undefined_source", {})


@pytest.mark.asyncio
async def test_create_source_ipc_directly_fails():
    """Test that create_source('ipc', ...) fails (removed from registry)."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="Unknown connector"):
        await c.create_source("ipc", {"call": "echo hello"})


@pytest.mark.asyncio
async def test_create_source_native_directly_fails():
    """Test that create_source('native', ...) fails (removed from registry)."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="Unknown connector"):
        await c.create_source("native", {"call": "python:mod:Class"})


@pytest.mark.asyncio
async def test_create_source_builtin_still_works():
    """Test that create_source('remote_dir', ...) still works."""
    c = await bundlebase.create(random_bundle())
    c = await c.create_source("remote_dir", {"url": "file:///some/path/"})
    assert c is not None


# ---- Drop source tests ----


@pytest.mark.asyncio
async def test_drop_connector_binding():
    """Test that drop_connector Python binding works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test")
    c = await c.drop_connector("test.my_source")
    assert c is not None


@pytest.mark.asyncio
async def test_drop_connector_removes_logic():
    """Test that drop_connector removes all logic entries."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test", "*/*")
    c = await c.drop_connector("test.my_source")

    # Re-defining the same connector should work (it was fully removed)
    c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test")
    assert c is not None


@pytest.mark.asyncio
async def test_drop_connector_undefined_fails():
    """Test that drop_connector fails for undefined connector."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="not defined"):
        await c.drop_connector("test.undefined_source")



# ---- Drop connector with platform tests ----


@pytest.mark.asyncio
async def test_drop_connector_with_platform():
    """Test that drop_connector with platform filter removes only that platform's logic."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test", "linux/amd64")
    c = await c.drop_connector("test.my_source", "linux/amd64")
    assert c is not None


@pytest.mark.asyncio
async def test_drop_temp_connector_logic_binding():
    """Test that drop_temp_connector_logic Python binding works on builder."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_temp_connector("test.my_source", "python", "mod:Class")
    result = await c.drop_temp_connector_logic("test.my_source")
    assert "Dropped 1 temporary connector logic" in result


@pytest.mark.asyncio
async def test_drop_temp_connector_logic_with_platform():
    """Test that drop_temp_connector_logic with platform filter works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_temp_connector("test.my_source", "python", "mod:Class", "linux/amd64")
    result = await c.drop_temp_connector_logic("test.my_source", "linux/amd64")
    assert "Dropped 1 temporary connector logic" in result


@pytest.mark.asyncio
async def test_drop_temp_connector_logic_on_bundle():
    """Test that drop_temp_connector_logic works on read-only bundle."""
    with tempfile.TemporaryDirectory() as path:
        c = await bundlebase.create(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        c = await c.import_connector("test.my_source", "ipc", "/usr/bin/test")
        await c.commit("Initial commit")

        bundle = await bundlebase.open(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        await bundle.import_temp_connector("test.my_source", "python", "mod:Class")
        result = await bundle.drop_temp_connector_logic("test.my_source")
        assert "Dropped 1 temporary connector logic" in result
