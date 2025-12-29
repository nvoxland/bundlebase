"""E2E tests for BundleConfig functionality."""

import maturin_import_hook
import pytest

maturin_import_hook.install()

import bundlebase
from conftest import random_bundle


@pytest.mark.asyncio
async def test_config_with_dict():
    """Test creating a container with config dict."""
    config = {
        "region": "us-west-2",
    }
    c = await bundlebase.create(random_bundle(), config=config)
    assert c is not None


@pytest.mark.asyncio
async def test_config_with_bundle_config():
    """Test creating a container with BundleConfig object."""
    config = bundlebase.BundleConfig()
    config.set("region", "us-west-2")
    config.set("endpoint", "http://localhost:9000", url_prefix="s3://test-bucket/")

    c = await bundlebase.create(random_bundle(), config=config)
    assert c is not None


@pytest.mark.asyncio
async def test_set_config_operation():
    """Test set_config operation for storing config in manifest."""
    c = await bundlebase.create(random_bundle())

    # Set some config values
    c = await c.set_config("region", "us-east-1")
    c = await c.set_config("endpoint", "http://localhost:9000", url_prefix="s3://test-bucket/")

    # Commit to persist
    commit = await c.commit("Add config settings")
    assert commit is not None


@pytest.mark.asyncio
async def test_config_with_url_overrides():
    """Test config with URL-specific overrides."""
    config = {
        "region": "us-west-2",  # Default for all S3
        "s3://test-bucket/": {
            "endpoint": "http://localhost:9000",
            "allow_http": "true"
        }
    }

    c = await bundlebase.create(random_bundle(), config=config)
    assert c is not None


@pytest.mark.asyncio
async def test_open_with_config():
    """Test opening a container with config."""
    path = random_bundle()

    # Create and commit
    c = await bundlebase.create(path)
    await c.commit("Initial commit")

    # Open with config
    config = {"region": "us-west-2"}
    c2 = await bundlebase.open(path, config=config)
    assert c2 is not None


@pytest.mark.asyncio
async def test_set_config_chaining():
    """Test that set_config supports fluent chaining."""
    c = await (bundlebase.create(random_bundle())
              .set_config("region", "us-west-2")
              .set_config("access_key_id", "TESTKEY"))

    assert c is not None


@pytest.mark.asyncio
async def test_config_none_is_valid():
    """Test that config=None works correctly (backward compatibility)."""
    # Test create with config=None
    c = await bundlebase.create(random_bundle(), config=None)
    assert c is not None

    # Test open with config=None on an existing bundle
    path = random_bundle()
    c = await bundlebase.create(path)
    await c.commit("Initial")

    c2 = await bundlebase.open(path, config=None)
    assert c2 is not None
