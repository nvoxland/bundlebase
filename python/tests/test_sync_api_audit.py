"""Regression tests for July 2026 audit findings in the sync wrapper (epic bb-cko).

Each test targets a sync-wrapper method that failed unconditionally before the
fix (verified against the built module):
  - SyncBundleBuilder.extend()      -> TypeError (PyBundle.extend descriptor on a builder)
  - SyncBundleBuilder.drop_function -> TypeError (3 args passed to a 2-arg binding)
  - SyncBundle.status()             -> AttributeError (PyBundle has no status)
"""

import pytest

import bundlebase.sync as bb
from conftest import datafile, random_bundle


def test_sync_builder_extend_returns_builder():
    """bb-cko: SyncBundleBuilder.extend() must return a new SyncBundleBuilder."""
    c = bb.create(random_bundle())
    c.attach(datafile("userdata.parquet"))
    c.commit("initial")

    extended = c.extend()

    assert isinstance(extended, bb.SyncBundleBuilder)
    # The extended builder is usable and sees the committed data.
    assert extended.num_rows() == 1000


def test_sync_drop_function_reaches_rust_layer():
    """bb-cko: drop_function must forward (name, platform) and hit the Rust layer.

    Before the fix it raised a Python TypeError from the extra input_types arg
    before ever reaching the binding. Dropping a missing function should raise a
    bundlebase error, not a TypeError.
    """
    c = bb.create(random_bundle())
    with pytest.raises(Exception) as excinfo:
        c.drop_function("nonexistent.fn")
    assert not isinstance(excinfo.value, TypeError), (
        f"drop_function raised a Python arg-mismatch TypeError: {excinfo.value}"
    )


def test_sync_builder_status_works():
    """bb-cko: status() must work on a builder (regression guard for the move)."""
    c = bb.create(random_bundle())
    c.attach(datafile("userdata.parquet"))
    status = c.status()
    assert status.change_count >= 1


def test_sync_opened_bundle_has_no_status():
    """bb-cko: read-only opened bundles have no uncommitted status.

    Matches the async API (PyBundle has no status()); status lives on builders.
    """
    path = random_bundle()
    c = bb.create(path)
    c.attach(datafile("userdata.parquet"))
    c.commit("initial")

    opened = bb.open(path)
    with pytest.raises(AttributeError):
        opened.status()

def test_sync_drop_temp_function_reaches_rust_layer():
    """bb-cko: drop_temp_function previously had NO Rust binding on either
    class — the sync wrapper delegated to a method that didn't exist. Dropping
    a nonexistent temp function must reach the Rust layer (bundlebase error or
    a drop-count message), never an AttributeError/TypeError from the missing
    binding.
    """
    c = bb.create(random_bundle())
    try:
        result = c.drop_temp_function("nonexistent.fn")
        # Some registries treat dropping a missing temp function as a no-op.
        assert "temporary function" in str(result)
    except Exception as e:
        assert not isinstance(e, (TypeError, AttributeError)), (
            f"drop_temp_function never reached the Rust layer: {e!r}"
        )


def test_sync_opened_bundle_drop_temp_function_binding_exists():
    """bb-cko: the opened-bundle class needs the binding too."""
    path = random_bundle()
    c = bb.create(path)
    c.attach(datafile("userdata.parquet"))
    c.commit("initial")

    opened = bb.open(path)
    try:
        result = opened.drop_temp_function("nonexistent.fn")
        assert "temporary function" in str(result)
    except Exception as e:
        assert not isinstance(e, (TypeError, AttributeError)), (
            f"drop_temp_function never reached the Rust layer: {e!r}"
        )
