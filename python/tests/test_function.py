"""Tests for Python bindings of function definition system.

Tests verify import_function, import_temp_function, and drop_function
operations via both the Python API and SQL command syntax, as well as
end-to-end Python UDF execution through the DataFusion bridge.
"""

import tempfile

import maturin_import_hook
import pytest

maturin_import_hook.install()

import bundlebase
from conftest import datafile, random_bundle

ALLOW_EXTERNAL_CODE_CONFIG = {"system": {"allow_external_code": "true"}}


@pytest.mark.asyncio
async def test_import_function_persistent_ipc():
    """Test that import_function with ipc runner records a persistent operation."""
    with tempfile.TemporaryDirectory() as path:
        c = await bundlebase.create(path, config=ALLOW_EXTERNAL_CODE_CONFIG)
        c = await c.import_function(
            "acme.double_val", ["Int64"], "Int64", "ipc", "./my_func"
        )
        assert c is not None

        # Verify the operation was recorded
        status = c.status()
        assert not status.is_empty()
        assert any(
            "IMPORT FUNCTION" in change.description for change in status.changes
        )

        # Commit and reopen to verify persistence
        await c.commit("Add function")
        reopened = await bundlebase.open(path, config=ALLOW_EXTERNAL_CODE_CONFIG)

        # Extend so we can inspect operations, then verify loadFunction is present
        builder = await reopened.extend()
        ops = builder.operations()
        assert any(op.op_type == "loadFunction" for op in ops)


@pytest.mark.asyncio
async def test_function_name_validation():
    """Test that function names must contain exactly one dot (namespace.name)."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)

    # Name without a dot should fail
    with pytest.raises((ValueError, Exception)):
        await c.import_function("double_val", ["Int64"], "Int64", "ipc", "./my_func")

    # Multi-level name (more than one dot) should fail
    with pytest.raises((ValueError, Exception)):
        await c.import_function(
            "acme.math.double_val", ["Int64"], "Int64", "ipc", "./my_func"
        )


@pytest.mark.asyncio
async def test_import_function_rejects_python_runner():
    """Test that persistent import_function rejects the python runner."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises(ValueError, match="python runner cannot be bundled"):
        await c.import_function(
            "acme.double_val", ["Int64"], "Int64", "python", "test_function:double_val"
        )


@pytest.mark.asyncio
async def test_import_temp_function():
    """Test that import_temp_function does not record a persistent operation."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_temp_function(
        "acme.double_val", ["Int64"], "Int64", "python", "test_function:double_val"
    )
    assert c is not None

    # Temporary functions should not appear in persistent operations
    status = c.status()
    has_import_function = any(
        "IMPORT FUNCTION" in change.description for change in status.changes
    )
    assert not has_import_function, (
        "Temporary function should not appear in persistent status changes"
    )


@pytest.mark.asyncio
async def test_drop_function():
    """Test that drop_function records the drop operation."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_function(
        "acme.double_val", ["Int64"], "Int64", "ipc", "./my_func"
    )
    c = await c.drop_function("acme.double_val")
    assert c is not None

    # Verify both create and drop operations are recorded
    status = c.status()
    descriptions = [change.description for change in status.changes]
    assert any("IMPORT FUNCTION" in d for d in descriptions)
    assert any("DROP FUNCTION" in d for d in descriptions)


@pytest.mark.asyncio
async def test_import_function_with_platform():
    """Test IMPORT FUNCTION with explicit platform."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_function(
        "acme.double_val", ["Int64"], "Int64", "ipc", "./my_func", "linux/amd64"
    )

    # Verify the operation was recorded with platform info
    status = c.status()
    assert any(
        "IMPORT FUNCTION" in change.description for change in status.changes
    )


@pytest.mark.asyncio
async def test_import_function_multiple_input_types():
    """Test IMPORT FUNCTION with multiple input types."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.import_function(
        "acme.concat_vals", ["Utf8", "Utf8"], "Utf8", "ipc", "./concat_func"
    )

    status = c.status()
    assert any(
        "IMPORT FUNCTION" in change.description for change in status.changes
    )


@pytest.mark.asyncio
async def test_function_type_validation_error():
    """Test that an invalid Arrow type name gives a clear error."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    with pytest.raises((ValueError, Exception), match="(?i)bad.?type|unknown|unsupported|invalid"):
        await c.import_function(
            "acme.bad_func", ["BadType"], "Int64", "ipc", "./my_func"
        )


@pytest.mark.asyncio
async def test_python_udf_scalar():
    """Test that a Python UDF can be invoked in a SQL query."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.double_val",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:double_val",
    )
    result = await c.query("SELECT id, test.double_val(id) as doubled FROM bundle LIMIT 5")
    df = await result.to_pandas()
    assert len(df) == 5
    for _, row in df.iterrows():
        assert row["doubled"] == row["id"] * 2


@pytest.mark.asyncio
async def test_python_udf_multi_arg():
    """Test a Python UDF with multiple arguments."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.add_vals",
        ["Int64", "Int64"],
        "Int64",
        "python",
        "test_function_helpers:add_vals",
    )
    result = await c.query("SELECT id, test.add_vals(id, id) as added FROM bundle LIMIT 5")
    df = await result.to_pandas()
    assert len(df) == 5
    for _, row in df.iterrows():
        assert row["added"] == row["id"] * 2


@pytest.mark.asyncio
async def test_python_udf_in_where_clause():
    """Test that a Python UDF works in WHERE clauses, not just SELECT."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.double_val",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:double_val",
    )

    result = await c.query(
        "SELECT id FROM bundle WHERE test.double_val(id) > 10 LIMIT 5"
    )
    df = await result.to_pandas()
    assert len(df) > 0
    # All returned ids should have double > 10, meaning id > 5
    for _, row in df.iterrows():
        assert row["id"] > 5


# ==================== Aggregate UDF tests ====================


@pytest.mark.asyncio
async def test_python_udaf_sum():
    """Test that a Python aggregate UDF computes a sum correctly."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.my_sum",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:MySum",
        function_type="aggregate",
    )

    # Compute the sum via our custom aggregate and compare with SQL SUM
    result = await c.query(
        "SELECT test.my_sum(id) as custom_sum, SUM(id) as builtin_sum FROM bundle"
    )
    df = await result.to_pandas()
    assert len(df) == 1
    assert df.iloc[0]["custom_sum"] == df.iloc[0]["builtin_sum"]


@pytest.mark.asyncio
async def test_python_udaf_in_group_by():
    """Test that a Python aggregate UDF works with GROUP BY."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.my_sum",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:MySum",
        function_type="aggregate",
    )

    result = await c.query(
        "SELECT gender, test.my_sum(id) as custom_sum, SUM(id) as builtin_sum "
        "FROM bundle GROUP BY gender ORDER BY gender"
    )
    df = await result.to_pandas()
    assert len(df) > 1  # Multiple groups
    for _, row in df.iterrows():
        assert row["custom_sum"] == row["builtin_sum"]


@pytest.mark.asyncio
async def test_python_udaf_as_window():
    """Test that a Python aggregate UDF works with OVER() clause as a window function."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.import_temp_function(
        "test.my_sum",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:MySum",
        function_type="aggregate",
    )

    result = await c.query(
        "SELECT id, test.my_sum(id) OVER (ORDER BY id) as running_sum "
        "FROM bundle ORDER BY id LIMIT 5"
    )
    df = await result.to_pandas()
    assert len(df) == 5
    # Running sum: first row should be id[0], second should be id[0]+id[1], etc.
    running = 0
    for _, row in df.iterrows():
        running += row["id"]
        assert row["running_sum"] == running


@pytest.mark.asyncio
async def test_load_aggregate_function_via_api():
    """Test creating an aggregate function via the Python API with function_type param."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))

    # Use the function_type parameter to create an aggregate function
    c = await c.import_temp_function(
        "test.my_sum",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:MySum",
        function_type="aggregate",
    )

    result = await c.query(
        "SELECT test.my_sum(id) as custom_sum, SUM(id) as builtin_sum FROM bundle"
    )
    df = await result.to_pandas()
    assert len(df) == 1
    assert df.iloc[0]["custom_sum"] == df.iloc[0]["builtin_sum"]


# ==================== Overloading tests ====================


@pytest.mark.asyncio
async def test_function_overloading_same_name_different_types():
    """Test that two functions with the same name but different types both work."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))

    # Register Int64 overload
    c = await c.import_temp_function(
        "test.double_val",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:double_val",
    )
    # Register Float64 overload (same name, different types)
    c = await c.import_temp_function(
        "test.double_val",
        ["Float64"],
        "Float64",
        "python",
        "test_function_helpers:double_val_float",
    )

    # Query with Int64 column
    result = await c.query(
        "SELECT id, test.double_val(id) as doubled FROM bundle LIMIT 5"
    )
    df = await result.to_pandas()
    assert len(df) == 5
    for _, row in df.iterrows():
        assert row["doubled"] == row["id"] * 2

    # Query with Float64 via CAST
    result = await c.query(
        "SELECT id, test.double_val(CAST(id AS FLOAT)) as doubled FROM bundle LIMIT 5"
    )
    df = await result.to_pandas()
    assert len(df) == 5
    for _, row in df.iterrows():
        assert abs(row["doubled"] - row["id"] * 2.0) < 0.01


@pytest.mark.asyncio
async def test_function_overloading_dispatch_correct_overload():
    """Test that overload dispatch picks the right overload based on input types."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))

    # Register Int64 overload (doubles)
    c = await c.import_temp_function(
        "test.double_val",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:double_val",
    )
    # Register Float64 overload
    c = await c.import_temp_function(
        "test.double_val",
        ["Float64"],
        "Float64",
        "python",
        "test_function_helpers:double_val_float",
    )

    # Int64 input should invoke the Int64 overload
    # Note: id column is Int32 in userdata.parquet, so cast to BIGINT (Int64)
    result = await c.query(
        "SELECT test.double_val(CAST(id AS BIGINT)) as d FROM bundle LIMIT 3"
    )
    df = await result.to_pandas()
    assert df["d"].dtype in ("int64", "Int64")

    # Float64 input should invoke the Float64 overload
    result = await c.query(
        "SELECT test.double_val(CAST(id AS DOUBLE)) as d FROM bundle LIMIT 3"
    )
    df = await result.to_pandas()
    assert df["d"].dtype == "float64"


@pytest.mark.asyncio
async def test_function_overloading_drop_with_platform():
    """Test dropping a function overload with platform filter."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))

    # Import function with platform
    c = await c.import_function(
        "test.platform_fn", ["Int64"], "Int64", "ipc", "./my_func", "linux/amd64"
    )
    # Drop only the linux/amd64 platform entry
    c = await c.drop_function("test.platform_fn", "linux/amd64")
    assert c is not None


@pytest.mark.asyncio
async def test_function_overloading_error_unsupported_types():
    """Test that calling a function with unsupported input types gives a clear error."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))

    # Register only Int64 overload
    c = await c.import_temp_function(
        "test.int_only",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:double_val",
    )

    # Calling with Utf8 should fail
    with pytest.raises(Exception):
        result = await c.query(
            "SELECT test.int_only(first_name) FROM bundle LIMIT 1"
        )
        await result.to_pandas()


@pytest.mark.asyncio
async def test_function_overloading_mixed_kinds_rejected():
    """Test that registering scalar and aggregate under the same name fails."""
    c = await bundlebase.create(random_bundle(), config=ALLOW_EXTERNAL_CODE_CONFIG)
    c = await c.attach(datafile("userdata.parquet"))

    # Register as scalar
    c = await c.import_temp_function(
        "test.mixed_func",
        ["Int64"],
        "Int64",
        "python",
        "test_function_helpers:double_val",
    )
    # Try to register same name as aggregate — should fail
    with pytest.raises((ValueError, Exception), match="(?i)mixed kinds"):
        await c.import_temp_function(
            "test.mixed_func",
            ["Int64"],
            "Int64",
            "python",
            "test_function_helpers:MySum",
            function_type="aggregate",
        )


# ==================== Auto-detection tests ====================


def test_bundlebase_metadata_convention():
    """Test that the bundlebase_metadata() function is correctly defined in the helper module."""
    import test_function_helpers

    metadata = test_function_helpers.bundlebase_metadata()
    assert "functions" in metadata
    functions = metadata["functions"]
    assert len(functions) == 3

    # Verify double_val entry
    double_val = next(f for f in functions if f["name"] == "double_val")
    assert double_val["input_types"] == ["Int64"]
    assert double_val["return_type"] == "Int64"
    assert double_val["kind"] == "scalar"

    # Verify add_vals entry
    add_vals = next(f for f in functions if f["name"] == "add_vals")
    assert add_vals["input_types"] == ["Int64", "Int64"]
    assert add_vals["return_type"] == "Int64"

    # Verify MySum entry
    my_sum = next(f for f in functions if f["name"] == "MySum")
    assert my_sum["input_types"] == ["Int64"]
    assert my_sum["return_type"] == "Int64"
    assert my_sum["kind"] == "aggregate"
