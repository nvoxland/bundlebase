"""Tests for terminal methods in OperationChain (BUG 1).

Terminal methods (commit, query, fetch, fetch_all) do NOT return a builder,
so they must execute the queued chain first, then call the method on the
resulting bundle and return ITS result (not queue them and `await result`).
"""

import tempfile

import pytest

import bundlebase
from conftest import datafile, random_bundle


@pytest.mark.asyncio
async def test_chain_commit_persists():
    """`await c.attach(f).commit("msg")` should execute attach then commit,
    persisting the data so a reopen sees the commit."""
    with tempfile.TemporaryDirectory() as tmpdir:
        c = await bundlebase.create(tmpdir)
        # attach then commit in a single chain; commit returns the same value
        # a direct `await builder.commit(...)` returns (the empty unit, ()).
        result = await c.attach(datafile("userdata.parquet")).commit("initial commit")
        assert result == ()

        reopened = await bundlebase.open(tmpdir)
        history = reopened.history()
        messages = [getattr(commit, "message", None) for commit in history]
        assert "initial commit" in messages
        assert await reopened.num_rows() == 1000


@pytest.mark.asyncio
async def test_chain_query_returns_queryresult():
    """`await c.attach(f).query(sql)` should return a usable QueryResult
    (the same wrapper a direct `await b.query(...)` returns)."""
    c = await bundlebase.create(random_bundle())
    result = await c.attach(datafile("userdata.parquet")).query(
        "SELECT * FROM bundle LIMIT 5"
    )
    # Must be the QueryResult wrapper (has to_pandas), not a raw stream
    assert isinstance(result, bundlebase.QueryResult)
    df = await result.to_pandas()
    assert len(df) == 5


@pytest.mark.asyncio
async def test_chain_op_after_attach_still_works():
    """Chaining a normal op after attach must still work (regression)."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet")).drop_column("email")
    schema_names = [f.name for f in await c.schema()]
    assert "email" not in schema_names
    assert await c.num_rows() == 1000


@pytest.mark.asyncio
async def test_chain_starting_then_fetch_is_terminal():
    """fetch is a terminal method: the chain executes the queued attach, then
    runs fetch on the resulting bundle. Proven here by fetch surfacing its OWN
    error ('No sources defined') rather than the chain queuing it and clobbering
    the builder (which would fail differently)."""
    c = await bundlebase.create(random_bundle())
    with pytest.raises(ValueError, match="No sources defined"):
        # attach starts the chain (mutation); fetch is terminal and runs on the
        # attached bundle, hitting fetch's real "no source on base pack" error.
        await c.attach(datafile("userdata.parquet")).fetch("base", "add")


@pytest.mark.asyncio
async def test_create_chain_commit():
    """CreateChain should also support terminal commit."""
    with tempfile.TemporaryDirectory() as tmpdir:
        result = await (
            bundlebase.create(tmpdir)
            .attach(datafile("userdata.parquet"))
            .commit("via create chain")
        )
        assert result == ()
        reopened = await bundlebase.open(tmpdir)
        messages = [getattr(c, "message", None) for c in reopened.history()]
        assert "via create chain" in messages


@pytest.mark.asyncio
async def test_extend_chain_query():
    """ExtendChain should support terminal query returning QueryResult."""
    with tempfile.TemporaryDirectory() as tmpdir:
        c = await bundlebase.create(tmpdir)
        c = await c.attach(datafile("userdata.parquet"))
        await c.commit("base")
        opened = await bundlebase.open(tmpdir)
        result = await opened.extend().query("SELECT * FROM bundle LIMIT 3")
        assert isinstance(result, bundlebase.QueryResult)
        df = await result.to_pandas()
        assert len(df) == 3


# bb-n5m: these builder mutations must be registered so they're chainable.
_CHAINABLE_MUTATIONS = [
    "create_report",
    "drop_report",
    "drop_function",
    "rename_function",
    "rename_connector",
    "import_temp_function",
]


def test_mutation_methods_are_chainable():
    """Every builder mutation the stubs advertise on chains must resolve
    through chain._ORIGINAL_METHODS, or chaining it raises AttributeError."""
    from bundlebase.chain import _ORIGINAL_METHODS

    missing = [m for m in _CHAINABLE_MUTATIONS if m not in _ORIGINAL_METHODS]
    assert missing == [], f"not registered for chaining: {missing}"


@pytest.mark.asyncio
async def test_chain_create_report_executes():
    """`c.attach(f).create_report(...)` must queue+run create_report, not raise
    AttributeError when the chain validates the method."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet")).create_report(
        "r1", "My Report", "a description", "# Heading"
    )
    # The chain resolved and executed; the builder came back.
    assert c is not None
