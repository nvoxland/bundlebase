"""Tests for Python bindings of connector registry operations.

These tests cover connector lifecycle: import, drop, rename, validation,
versioning, and persistence. Source creation and data querying tests
live in test_source.py.
"""

import tempfile

import pytest

import bundlebase
from conftest import random_bundle


ALLOW_EXTERNAL_CODE_CONFIG = {"system": {"allow_external_code": "true"}}


# ---- Import connector tests ----


@pytest.mark.asyncio
async def test_import_connector_binding():
    """Test that import_connector Python binding works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "docker::test-connector-image")
    assert c is not None


@pytest.mark.asyncio
async def test_import_temp_connector_binding():
    """Test that import_temp_connector Python binding works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_temp_connector("test.my_source", "python::test_function_helpers:double_val")
    assert c is not None


@pytest.mark.asyncio
async def test_import_temp_connector_rejects_nonexistent_ipc():
    """Test that import_temp_connector fails when the IPC binary doesn't exist."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises((ValueError, Exception), match="not found"):
        await c.import_temp_connector(
            "test.my_source", "ipc::./nonexistent_binary_xyz"
        )


@pytest.mark.asyncio
async def test_import_temp_connector_rejects_nonexistent_python():
    """Test that import_temp_connector fails when the Python module doesn't exist."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises((ValueError, Exception)):
        await c.import_temp_connector(
            "test.my_source", "python::nonexistent_module_xyz:Class"
        )


@pytest.mark.asyncio
async def test_import_temp_connector_builder_version_uncommitted_temp():
    """Test that import_temp_connector on builder with changes sets version to UNCOMMITTED+TEMP."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    assert c.version != "UNCOMMITTED+TEMP"
    # Add a persistent change so version becomes UNCOMMITTED
    c = await c.import_connector("test.my_source", "docker::test-connector-image")
    assert c.version == "UNCOMMITTED"
    # Now add a temporary connector to get UNCOMMITTED+TEMP
    c = await c.import_temp_connector("test.temp_source", "python::test_function_helpers:double_val")
    assert c.version == "UNCOMMITTED+TEMP"


@pytest.mark.asyncio
async def test_import_temp_connector_bundle_version_temp():
    """Test that import_temp_connector on read-only bundle sets version to TEMP."""
    with tempfile.TemporaryDirectory() as path:
        c = await bundlebase.create(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        c = await c.import_connector("test.my_source", "docker::test-connector-image")
        await c.commit("Initial commit")

        bundle = await bundlebase.open(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        assert bundle.version != "TEMP"
        await bundle.import_temp_connector("test.my_source", "python::test_function_helpers:double_val")
        assert bundle.version == "TEMP"


@pytest.mark.asyncio
async def test_import_connector_rejects_python_calls():
    """Test that import_connector rejects python runtime."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="'python' runtime cannot be bundled"):
        await c.import_connector("test.my_source", "python::test_function_helpers:double_val")


@pytest.mark.asyncio
async def test_import_connector_success_with_ipc():
    """Test that import_connector succeeds with ipc runner."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "docker::test-connector-image")
    assert c is not None


# ---- Persistence tests ----


@pytest.mark.asyncio
async def test_import_connector_survives_commit_reopen():
    """Test that import_connector persists through commit and reopen."""
    with tempfile.TemporaryDirectory() as path:
        c = await bundlebase.create(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        c = await c.import_connector("test.my_source", "docker::test-connector-image")
        await c.commit("Add source definition")

        # Reopen and verify the source definition survived
        bundle = await bundlebase.open(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        builder = await bundle.extend()
        # Should not raise "not defined" — the source should exist
        c = await builder.drop_connector("test.my_source")
        assert c is not None


# ---- Drop connector tests ----


@pytest.mark.asyncio
async def test_drop_connector_binding():
    """Test that drop_connector Python binding works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "docker::test-connector-image")
    c = await c.drop_connector("test.my_source")
    assert c is not None


@pytest.mark.asyncio
async def test_drop_connector_removes_logic():
    """Test that drop_connector removes all logic entries."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "docker::test-connector-image", "*/*")
    c = await c.drop_connector("test.my_source")

    # Re-defining the same connector should work (it was fully removed)
    c = await c.import_connector("test.my_source", "docker::test-connector-image")
    assert c is not None


@pytest.mark.asyncio
async def test_drop_connector_undefined_fails():
    """Test that drop_connector fails for undefined connector."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="not defined"):
        await c.drop_connector("test.undefined_source")


@pytest.mark.asyncio
async def test_drop_connector_with_platform():
    """Test that drop_connector with platform filter removes only that platform's logic."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "docker::test-connector-image", "linux/amd64")
    c = await c.drop_connector("test.my_source", "linux/amd64")
    assert c is not None


@pytest.mark.asyncio
async def test_drop_connector_with_platform_filter():
    """Test that drop_connector with platform filter only removes matching entry."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    # Import for two platforms
    c = await c.import_connector("test.multi_plat", "docker::test-linux-image", "linux/amd64")
    c = await c.import_connector("test.multi_plat", "docker::test-darwin-image", "darwin/arm64")
    # Drop only linux
    c = await c.drop_connector("test.multi_plat", "linux/amd64")
    # The connector should still exist (darwin entry remains)
    # Re-adding linux should work
    c = await c.import_connector("test.multi_plat", "docker::test-linux-image2", "linux/amd64")
    assert c is not None


@pytest.mark.asyncio
async def test_drop_temp_connector_binding():
    """Test that drop_temp_connector Python binding works on builder."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_temp_connector("test.my_source", "python::test_function_helpers:double_val")
    result = await c.drop_temp_connector("test.my_source")
    assert "Dropped 1 temporary connector" in result


@pytest.mark.asyncio
async def test_drop_temp_connector_with_platform():
    """Test that drop_temp_connector with platform filter works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_temp_connector("test.my_source", "python::test_function_helpers:double_val", "linux/amd64")
    result = await c.drop_temp_connector("test.my_source", "linux/amd64")
    assert "Dropped 1 temporary connector" in result


@pytest.mark.asyncio
async def test_drop_temp_connector_on_bundle():
    """Test that drop_temp_connector works on read-only bundle."""
    with tempfile.TemporaryDirectory() as path:
        c = await bundlebase.create(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        c = await c.import_connector("test.my_source", "docker::test-connector-image")
        await c.commit("Initial commit")

        bundle = await bundlebase.open(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        await bundle.import_temp_connector("test.my_source", "python::test_function_helpers:double_val")
        result = await bundle.drop_temp_connector("test.my_source")
        assert "Dropped 1 temporary connector" in result


# ---- Naming validation tests ----


@pytest.mark.asyncio
async def test_connector_name_no_dot_fails():
    """Test that connector name without a dot fails."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises((ValueError, Exception)):
        await c.import_connector("no_dot_name", "docker::test-connector-image")


@pytest.mark.asyncio
async def test_connector_name_too_many_dots_fails():
    """Test that connector name with too many dots fails."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises((ValueError, Exception)):
        await c.import_connector("a.b.c", "docker::test-connector-image")


# ---- Rename connector tests ----


@pytest.mark.asyncio
async def test_rename_connector_binding():
    """Test that rename_connector Python binding works."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.my_source", "docker::test-connector-image")
    c = await c.rename_connector("test.my_source", "test.renamed_source")
    assert c is not None


@pytest.mark.asyncio
async def test_rename_connector_undefined_fails():
    """Test that rename_connector fails for undefined connector."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="not defined"):
        await c.rename_connector("test.undefined_source", "test.new_name")


@pytest.mark.asyncio
async def test_rename_connector_to_existing_fails():
    """Test that rename_connector fails when target name already exists."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_connector("test.source_a", "docker::test-image-a")
    c = await c.import_connector("test.source_b", "docker::test-image-b")
    with pytest.raises(ValueError, match="already exists"):
        await c.rename_connector("test.source_a", "test.source_b")
