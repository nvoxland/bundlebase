"""Tests for file-backed Python functions (python::./script.py:func).

File-backed Python functions are executed via IPC subprocess using the
bundlebase_sdk._ipc_harness module, unlike module-backed Python functions
which use the in-process PyO3 bridge.
"""

import os
import tempfile

import pytest

import bundlebase
from conftest import datafile, random_bundle

ALLOW_EXTERNAL_CODE_CONFIG = {"system": {"allow_external_code": "true"}}

# Reusable script content for a scalar function with metadata
SCALAR_SCRIPT = """\
import pyarrow as pa
import pyarrow.compute as pc


def double_val(col: pa.Array) -> pa.Array:
    return pc.multiply(col, 2)


def bundlebase_metadata():
    return {
        "functions": [
            {
                "name": "double_val",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            },
        ]
    }
"""

AGGREGATE_SCRIPT = """\
import pyarrow as pa
import pyarrow.compute as pc


def double_val(col: pa.Array) -> pa.Array:
    return pc.multiply(col, 2)


class MySum:
    def create_state(self):
        return pa.scalar(0, type=pa.int64())

    def accumulate(self, state, values):
        batch_sum = pc.sum(values).as_py()
        if batch_sum is None:
            return state
        return pa.scalar(state.as_py() + batch_sum, type=pa.int64())

    def merge(self, state1, state2):
        return pa.scalar(state1.as_py() + state2.as_py(), type=pa.int64())

    def evaluate(self, state):
        return state


def bundlebase_metadata():
    return {
        "functions": [
            {
                "name": "double_val",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "scalar",
            },
            {
                "name": "MySum",
                "input_types": ["Int64"],
                "return_type": "Int64",
                "kind": "aggregate",
            },
        ]
    }
"""


@pytest.fixture
def scalar_script():
    """Create a temporary .py file with a scalar function."""
    with tempfile.NamedTemporaryFile(
        suffix=".py", mode="w", delete=False, dir="."
    ) as f:
        f.write(SCALAR_SCRIPT)
        path = f.name
    yield path
    os.unlink(path)


@pytest.fixture
def aggregate_script():
    """Create a temporary .py file with both scalar and aggregate functions."""
    with tempfile.NamedTemporaryFile(
        suffix=".py", mode="w", delete=False, dir="."
    ) as f:
        f.write(AGGREGATE_SCRIPT)
        path = f.name
    yield path
    os.unlink(path)


# ==================== Detection tests ====================


@pytest.mark.asyncio
async def test_file_backed_python_is_bundleable(scalar_script):
    """Test that file-backed Python (./script.py) can be bundled."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    # Should NOT raise "cannot be bundled"
    c = await c.import_function(
        "test.double_val", f"python::{scalar_script}:double_val"
    )
    assert c is not None


@pytest.mark.asyncio
async def test_module_backed_python_still_not_bundleable():
    """Test that module-backed Python (mymodule:func) still cannot be bundled."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="'python' runtime cannot be bundled"):
        await c.import_function(
            "acme.double_val", "python::test_function_helpers:double_val"
        )


# ==================== Temp function tests ====================


@pytest.mark.asyncio
async def test_file_backed_temp_scalar(scalar_script):
    """Test that a file-backed Python scalar function works via import_temp_function."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.double_val", f"python::{scalar_script}:double_val"
    )
    result = await c.query(
        "SELECT id, test.double_val(id) as doubled FROM bundle LIMIT 5"
    )
    df = await result.to_pandas()
    assert len(df) == 5
    for _, row in df.iterrows():
        assert row["doubled"] == row["id"] * 2


@pytest.mark.asyncio
async def test_file_backed_temp_aggregate(aggregate_script):
    """Test that a file-backed Python aggregate function works via import_temp_function."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.my_sum", f"python::{aggregate_script}:MySum"
    )
    result = await c.query(
        "SELECT test.my_sum(id) as custom_sum, SUM(id) as builtin_sum FROM bundle"
    )
    df = await result.to_pandas()
    assert len(df) == 1
    assert df.iloc[0]["custom_sum"] == df.iloc[0]["builtin_sum"]


# ==================== Persistent function tests ====================


@pytest.mark.asyncio
async def test_file_backed_persistent_scalar(scalar_script):
    """Test that a file-backed Python function can be persisted, committed, and queried after reopen."""
    with tempfile.TemporaryDirectory() as temp_dir:
        # Create bundle with a file-backed function
        c = await bundlebase.create(temp_dir, config=ALLOW_EXTERNAL_CODE_CONFIG)
        c = await c.attach(datafile("userdata.parquet"))
        c = await c.import_function(
            "test.double_val", f"python::{scalar_script}:double_val"
        )

        # Query before commit
        result = await c.query(
            "SELECT id, test.double_val(id) as doubled FROM bundle LIMIT 5"
        )
        df = await result.to_pandas()
        assert len(df) == 5
        for _, row in df.iterrows():
            assert row["doubled"] == row["id"] * 2

        # Commit and reopen
        await c.commit("Add file-backed Python function")
        c2 = await bundlebase.open(temp_dir, config=ALLOW_EXTERNAL_CODE_CONFIG)

        # Query after reopen — function should still work from bundled .py
        result2 = await c2.query(
            "SELECT id, test.double_val(id) as doubled FROM bundle LIMIT 5"
        )
        df2 = await result2.to_pandas()
        assert len(df2) == 5
        for _, row in df2.iterrows():
            assert row["doubled"] == row["id"] * 2


# ==================== Validation tests ====================


@pytest.mark.asyncio
async def test_file_backed_nonexistent_script():
    """Test that import_temp_function fails when the .py file doesn't exist."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises((ValueError, Exception), match="not found"):
        await c.import_temp_function(
            "acme.func", "python::./nonexistent_xyz.py:func"
        )
