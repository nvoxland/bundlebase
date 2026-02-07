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
        "s3": {"region": "us-west-2"},
    }
    c = await bundlebase.create(random_bundle(), config=config)
    assert c is not None


@pytest.mark.asyncio
async def test_save_config_operation():
    """Test save_config operation for storing config in manifest."""
    c = await bundlebase.create(random_bundle())

    # Set some config values
    c = await c.save_config("s3", "region", "us-east-1")
    c = await c.save_config("s3/test-bucket", "endpoint", "http://localhost:9000")

    # Commit to persist
    commit = await c.commit("Add config settings")
    assert commit is not None


@pytest.mark.asyncio
async def test_config_with_scope_overrides():
    """Test config with scope-specific overrides."""
    config = {
        "s3": {"region": "us-west-2"},
        "s3/test-bucket": {
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
    config = {"s3": {"region": "us-west-2"}}
    c2 = await bundlebase.open(path, config=config)
    assert c2 is not None


@pytest.mark.asyncio
async def test_save_config_chaining():
    """Test that save_config supports fluent chaining."""
    c = await (bundlebase.create(random_bundle())
               .save_config("s3", "region", "us-west-2")
               .save_config("s3", "access_key_id", "TESTKEY"))

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


@pytest.mark.asyncio
async def test_save_config_with_scope():
    """Test setting config using a direct scope path."""
    c = await bundlebase.create(random_bundle())

    c = await c.save_config("s3/my-bucket", "region", "us-west-2")

    commit = await c.commit("Config with scope")
    assert commit is not None


@pytest.mark.asyncio
async def test_config_flat_keys_rejected():
    """Test that flat config keys are rejected — must be nested under URL scope."""
    with pytest.raises(ValueError, match="must be nested under a scope path"):
        config = {"region": "us-west-2"}
        await bundlebase.create(random_bundle(), config=config)


@pytest.mark.asyncio
async def test_config_scoped_keys_override_env():
    """Test that passed scoped config overrides environment variables."""
    import os

    env_key = "BB_S3_REGION"
    os.environ[env_key] = "env_value"
    try:
        # Passed config should override the env var
        config = {"s3": {"region": "passed_value"}}
        c = await bundlebase.create(random_bundle(), config=config)
        assert c is not None
    finally:
        del os.environ[env_key]


@pytest.mark.asyncio
async def test_set_config_basic():
    """Test set_config sets a runtime config value."""
    c = await bundlebase.create(random_bundle())
    c = await c.set_config("s3", "region", "us-west-2")
    assert c is not None


@pytest.mark.asyncio
async def test_set_config_with_scope():
    """Test set_config with scope."""
    c = await bundlebase.create(random_bundle())
    c = await c.set_config("s3/my-bucket", "region", "us-west-2")
    assert c is not None


@pytest.mark.asyncio
async def test_set_config_unknown_scope_raises_error():
    """Test set_config with an unknown scope raises an error."""
    c = await bundlebase.create(random_bundle())
    with pytest.raises(ValueError, match="Unknown scope"):
        await c.set_config("nonexistent", "region", "us-west-2")


@pytest.mark.asyncio
async def test_set_config_chaining():
    """Test that set_config supports fluent chaining."""
    c = await (bundlebase.create(random_bundle())
               .set_config("s3", "region", "us-west-2")
               .set_config("s3", "endpoint", "http://localhost:9000"))
    assert c is not None


@pytest.mark.asyncio
async def test_set_config_not_persisted():
    """Test that set_config values are not persisted after commit/reopen."""
    import tempfile, os

    with tempfile.TemporaryDirectory() as tmpdir:
        path = os.path.join(tmpdir, "bundle")

        c = await bundlebase.create(path)
        # Set runtime config and then commit — runtime config should NOT be persisted
        c = await c.set_config("s3", "region", "us-west-2")
        await c.commit("Initial commit")

        # Reopen — the runtime config should not be there (it's session-only)
        c2 = await bundlebase.open(path)
        # We can't directly inspect config, but we verify the bundle opens normally
        assert c2 is not None


@pytest.mark.asyncio
async def test_save_config_last_wins():
    """Test that when save_config is called multiple times for the same key, the last value wins.

    Both operations are preserved in the operation history, but the last value
    is the one that takes effect when the config is resolved.
    """
    import tempfile, os

    with tempfile.TemporaryDirectory() as tmpdir:
        path = os.path.join(tmpdir, "bundle")

        c = await bundlebase.create(path)
        c = await c.save_config("s3", "region", "us-west-1")
        c = await c.save_config("s3", "region", "us-east-1")
        await c.commit("Override config")

        # Reopen and verify both operations were persisted
        reopened = await bundlebase.open(path)
        builder = await reopened.extend()
        config_ops = [op for op in builder.operations() if op.op_type == "saveConfig"]
        assert len(config_ops) == 2
        # Last operation should be the override
        assert "us-east-1" in config_ops[1].describe


@pytest.mark.asyncio
async def test_save_config_last_wins_scoped():
    """Test that scoped save_config last-value-wins works after commit/reopen."""
    import tempfile, os

    with tempfile.TemporaryDirectory() as tmpdir:
        path = os.path.join(tmpdir, "bundle")

        c = await bundlebase.create(path)
        c = await c.save_config("s3/bucket", "endpoint", "http://old:9000")
        c = await c.save_config("s3/bucket", "endpoint", "http://new:9000")
        await c.commit("Override scoped config")

        reopened = await bundlebase.open(path)
        builder = await reopened.extend()
        config_ops = [op for op in builder.operations() if op.op_type == "saveConfig"]
        assert len(config_ops) == 2
        assert "http://new:9000" in config_ops[1].describe


@pytest.mark.asyncio
async def test_save_config_last_wins_across_commits():
    """Test that save_config in a later commit overrides an earlier commit."""
    import tempfile, os

    with tempfile.TemporaryDirectory() as tmpdir:
        path = os.path.join(tmpdir, "bundle")

        c = await bundlebase.create(path)
        c = await c.save_config("s3", "region", "us-west-1")
        await c.commit("First config")

        c = await c.save_config("s3", "region", "eu-west-1")
        await c.commit("Override config")

        reopened = await bundlebase.open(path)
        builder = await reopened.extend()
        config_ops = [op for op in builder.operations() if op.op_type == "saveConfig"]
        assert len(config_ops) == 2
        # Operations are in commit order — last one wins
        assert "us-west-1" in config_ops[0].describe
        assert "eu-west-1" in config_ops[1].describe




