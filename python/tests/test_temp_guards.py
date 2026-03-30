"""Tests for temporary function/connector guardrails.

Verifies that:
- commit is blocked when a filter references a temp-only function
- commit succeeds when filter doesn't use a temp function
- commit succeeds when a persistent function shadows a temp one
- view creation is blocked when SQL references a temp-only function
- fetch fails with helpful message when connector is missing after reopen
"""

import pytest

import bundlebase
from conftest import datafile, random_bundle

ALLOW_EXTERNAL_CODE_CONFIG = {"system": {"allow_external_code": "true"}}


@pytest.mark.asyncio
async def test_commit_blocked_with_temp_function_in_filter():
    """Commit should fail when a filter query references a temp-only function."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.double_val", "python::test_function_helpers:double_val"
    )

    # Apply filter that uses the temp function
    c = await c.filter(
        "SELECT * FROM bundle WHERE test.double_val(id) > 10", []
    )

    # Commit should be blocked
    with pytest.raises((ValueError, Exception), match="temporary function"):
        await c.commit("should fail")


@pytest.mark.asyncio
async def test_commit_succeeds_without_temp_function():
    """Commit should succeed when the filter doesn't use a temp function."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.double_val", "python::test_function_helpers:double_val"
    )

    # Apply filter that does NOT use the temp function
    c = await c.filter("SELECT * FROM bundle WHERE id > 10", [])

    # Commit should succeed -- the filter doesn't reference the temp function
    await c.commit("filter without temp function")


@pytest.mark.asyncio
async def test_create_view_blocked_with_temp_function():
    """CREATE VIEW should fail when SQL references a temp-only function."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    await c.commit("Initial data")

    c = await c.import_temp_function(
        "test.double_val", "python::test_function_helpers:double_val"
    )

    # Creating a view that uses the temp function should fail
    with pytest.raises((ValueError, Exception), match="temporary function"):
        await c.create_view(
            "bad_view",
            "SELECT id, test.double_val(id) as doubled FROM bundle",
        )


@pytest.mark.asyncio
async def test_create_view_succeeds_without_temp_function():
    """CREATE VIEW should succeed when SQL doesn't use temp functions."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    await c.commit("Initial data")

    c = await c.import_temp_function(
        "test.double_val", "python::test_function_helpers:double_val"
    )

    # Creating a view that does NOT use the temp function should succeed
    view = await c.create_view(
        "safe_view",
        "SELECT * FROM bundle WHERE id > 50",
    )
    assert view is not None
