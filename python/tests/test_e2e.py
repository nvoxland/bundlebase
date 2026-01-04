import os
import tempfile

import maturin_import_hook
import polars
import pyarrow as pa
import pytest
import yaml

maturin_import_hook.install()

import bundlebase
from conftest import datafile, random_bundle


@pytest.mark.asyncio
async def test_empty_bundle():
    c = await bundlebase.create(random_bundle())
    assert c is not None
    assert (await c.schema()).is_empty()
    assert len((await c.schema())) == 0

    assert await c.num_rows() == 0


@pytest.mark.asyncio
async def test_parquet_support():
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Verify schema is populated
    assert len((await c.schema())) == 13
    assert await c.num_rows() == 1000  # num_rows() async method

    # Verify conversion works
    results: polars.DataFrame = await c.to_polars()
    assert len(results) == 1000


@pytest.mark.asyncio
async def test_csv_support():
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))

    # Verify schema is populated
    assert len((await c.schema())) == 12
    assert await c.num_rows() == 100

    # Verify conversion works
    results = await c.to_polars()
    assert len(results) == 100


@pytest.mark.asyncio
async def test_json_support():
    """Test that JSON binding works correctly"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("objects.json"))

    # Verify schema is populated
    assert len((await c.schema())) == 4
    assert await c.num_rows() == 4

    # Verify conversion works
    results = await c.to_polars()
    assert len(results) == 4


@pytest.mark.asyncio
async def test_chaining():
    c = await (bundlebase.create(random_bundle())
               .attach(datafile("userdata.parquet"))
               .remove_column("country")
               .rename_column("title", "new_title"))

    assert "new_title" in [f.name for f in (await c.schema()).fields]
    assert "country" not in [f.name for f in (await c.schema()).fields]


@pytest.mark.asyncio
async def test_custom_functions():
    c = await bundlebase.create(random_bundle())

    def my_data(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
        if page == 0:
            return pa.record_batch(
                schema=schema,
                data={
                    "id": [1, 2, 3],
                    "name": ["Alice", "Bob", "Charlie"]
                }
            )
        return None

    c = await c.define_function(
        name="test_function",
        output={
            "id": "Int64",
            "name": "Utf8",
        },
        func=my_data,
        version="2"
    )

    c = await c.attach("function://test_function")
    df = await c.to_pandas()
    assert df["name"].tolist() == ["Alice", "Bob", "Charlie"]
    assert df["id"].tolist() == [1, 2, 3]


@pytest.mark.asyncio
async def test_open_save():
    """Test that save/open roundtrip works correctly"""
    import tempfile

    # Create a bundle with data and transformations
    with tempfile.TemporaryDirectory() as temp_dir:
        c = await bundlebase.create(temp_dir)
        c = await c.set_name("Test Bundle")
        c = await c.attach(datafile("userdata.parquet"))

        # Save and reload
        await c.commit("Commit changes")
        loaded_c = await bundlebase.open(temp_dir)

        # Verify metadata was preserved
        assert loaded_c.name == c.name

        # Verify data can be queried
        loaded_results = await loaded_c.to_dict()
        assert loaded_results is not None

        # Verify new operations: history(), url()
        # URL is returned as a file:// URL
        assert temp_dir in loaded_c.url
        history = loaded_c.history()
        assert len(history) >= 1
        assert any(h.message == "Commit changes" for h in history)

        # Verify commit details
        commit = history[0]
        assert commit.author is not None
        assert commit.timestamp is not None
        assert len(commit.operations) >= 1


@pytest.mark.asyncio
async def test_name():
    c = await bundlebase.create(random_bundle())
    # default should be None / not set
    assert c.name is None

    # set name and verify getter
    await c.set_name("My Bundle")
    assert c.name == "My Bundle"


@pytest.mark.asyncio
async def test_description():
    """Test setting and getting bundle description"""
    c = await bundlebase.create(random_bundle())

    # Default should be None
    assert c.description is None

    # Set description and verify getter
    await c.set_description("This is a test bundle")
    assert c.description == "This is a test bundle"


@pytest.mark.asyncio
async def test_join():
    """Test that join() method binding works correctly"""
    c = await (bundlebase.create(random_bundle())
               .attach(datafile("customers-0-100.csv"))
               .join("regions", datafile("sales-regions.csv"), '$base."Country" = regions."Country"'))

    assert await c.num_rows() == 99

    await c.attach_to_join("regions", datafile("sales-regions-2.csv"))
    assert await c.num_rows() == 100


@pytest.mark.asyncio
async def test_schema():
    c = await bundlebase.create(random_bundle())
    # Empty schema returns empty string
    schema = await c.schema()
    assert str(schema) == ""

    c = await c.attach(datafile("userdata.parquet"))
    schema = await c.schema()
    assert len(schema) == 13

    # Check field names
    field_names = [f.name for f in schema.fields]
    assert "id" in field_names
    assert "first_name" in field_names
    assert "email" in field_names
    assert len(field_names) == 13
    assert "first_name" in str(schema)

    assert schema.field("id").data_type == pa.int32()
    assert schema.field("id").name == "id"
    assert schema.field("id").nullable
    assert str(schema.field("first_name")) == "first_name: Utf8View"

    with pytest.raises(ValueError, match="Schema error: Unable to get field named \"invalid\". Valid fields:"):
        schema.field("invalid")


@pytest.mark.asyncio
async def test_version():
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Version should be a 12-character hex string
    assert isinstance(c.version, str)
    assert len(c.version) == 12
    assert all(c in '0123456789abcdef' for c in c.version)


@pytest.mark.asyncio
async def test_select():
    c = await bundlebase.create(random_bundle())
    c = await (c.attach(datafile("userdata.parquet"))
               .filter("salary > $1", [50000.0])
               .select("id", "salary"))

    results = await c.to_dict()
    # Filter should reduce rows, select should limit columns
    assert "id" in results
    assert "salary" in results
    assert len(results["id"]) == 798  # 798 rows with salary > 50000


@pytest.mark.asyncio
async def test_select():
    c = await (bundlebase.create().attach(datafile("userdata.parquet")))
    q = c.select("SELECT * FROM data LIMIT 10")

    results = await q.to_dict()
    assert len(results["id"]) == 10


@pytest.mark.asyncio
async def test_filter():
    c = await bundlebase.create(random_bundle())
    c = await (c.attach(datafile("userdata.parquet"))
               .filter("salary > $1", [50000.0]))

    results = await c.to_dict()
    assert len(results["id"]) == 798


@pytest.mark.asyncio
async def test_python_function_with_multiple_pages():
    """Test Python function that returns data across multiple pages"""
    c = await bundlebase.create(random_bundle())

    def paginated_data(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
        if page == 0:
            return pa.record_batch(
                schema=schema,
                data={"page_num": [0, 0]}
            )
        elif page == 1:
            return pa.record_batch(
                schema=schema,
                data={"page_num": [1, 1]}
            )
        return None

    c = await c.define_function(
        name="paginated_func",
        output={"page_num": "Int32"},
        func=paginated_data,
        version="3",
    )

    c = await c.attach("function://paginated_func")
    results = await c.to_dict()

    # Both pages should be combined
    total_rows = len(list(results.values())[0]) if results else 0
    assert total_rows == 4, f"Expected 4 total rows, got {total_rows}"


@pytest.mark.asyncio
async def test_python_function_error_handling():
    """Test error handling for Python functions"""
    c = await bundlebase.create(random_bundle())

    def error_data(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
        if page == 0:
            return pa.record_batch(
                schema=schema,
                data={"id": [1]}
            )
        return None

    # Define function with mismatched schema should still work (schema validation happens at attach time)
    c = await c.define_function(
        name="error_func",
        output={"id": "Int64"},
        func=error_data,
        version="3",
    )

    # Attach and query should work
    c = await c.attach("function://error_func")
    results = await c.to_dict()
    row_count = len(list(results.values())[0]) if results else 0
    assert row_count == 1


@pytest.mark.asyncio
async def test_to_pandas():
    """Test conversion to pandas DataFrame"""
    c = await bundlebase.create(random_bundle())

    with pytest.raises(ValueError, match="no data"):
        await c.to_pandas()

    c = await c.attach(datafile("userdata.parquet"))

    # Export to pandas using standalone function
    df = await c.to_pandas()

    # Verify it's a pandas DataFrame
    assert hasattr(df, "shape")
    assert df.shape[0] == 1000
    assert "id" in df.columns
    assert "first_name" in df.columns


@pytest.mark.asyncio
async def test_to_polars():
    c = await bundlebase.create(random_bundle())
    with pytest.raises(ValueError, match="no data"):
        await c.to_dict()

    c = await c.attach(datafile("userdata.parquet"))

    df = await c.to_polars()

    assert hasattr(df, "shape")
    assert df.shape[0] == 1000  # Should have 1000 rows
    assert "id" in df.columns
    assert "first_name" in df.columns


@pytest.mark.asyncio
async def test_to_numpy():
    """Test conversion to dict of numpy arrays"""

    c = await bundlebase.create(random_bundle())
    with pytest.raises(ValueError, match="no data"):
        await c.to_dict()

    c = await c.attach(datafile("userdata.parquet"))

    arrays = await c.to_numpy()

    assert isinstance(arrays, dict)
    assert "id" in arrays
    assert "first_name" in arrays
    assert len(arrays["id"]) == 1000


@pytest.mark.asyncio
async def test_to_dict():
    """Test conversion to dict of lists"""
    c = await bundlebase.create(random_bundle())

    with pytest.raises(ValueError, match="no data"):
        await c.to_dict()

    c = await c.attach(datafile("userdata.parquet"))

    data_dict = await c.to_dict()

    assert isinstance(data_dict, dict)
    assert "id" in data_dict
    assert "first_name" in data_dict
    assert len(data_dict["id"]) == 1000


@pytest.mark.asyncio
async def test_explain():
    """Test query plan explanation"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Explain should return a non-empty string with formatted plan type markers
    plan = await c.explain()
    assert isinstance(plan, str)
    assert len(plan) > 0
    # Should contain plan type markers (*** PLAN_TYPE ***)
    assert "***" in plan


@pytest.mark.asyncio
async def test_explain_with_filter():
    """Test query plan explanation with filters"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))
    c = await c.filter("salary > $1", [50000.0])

    # Explain should return a plan with the filter applied
    plan = await c.explain()
    assert isinstance(plan, str)
    assert len(plan) > 0


@pytest.mark.asyncio
async def test_extend_bundle_basic():
    """Test extending a bundle to a new directory"""
    import tempfile

    with tempfile.TemporaryDirectory() as temp1:
        with tempfile.TemporaryDirectory() as temp2:
            # Create and commit first bundle
            c1 = await bundlebase.create(temp1)
            c1 = await c1.set_name("Test Bundle")
            c1 = await c1.attach(datafile("userdata.parquet"))
            await c1.commit("Initial commit")

            # Open first bundle
            c_opened = await bundlebase.open(temp1)

            # Verify first bundle is still intact
            schema_opened = await c_opened.schema()
            assert "country" in [f.name for f in schema_opened.fields]
            assert await c_opened.num_rows() == 1000

            # Extend to a new directory
            c_extended = await c_opened.extend(temp2)

            # Verify the extended bundle has the same data
            assert await c_extended.num_rows() == 1000
            schema_extended = await c_extended.schema()
            assert "country" in [f.name for f in schema_extended.fields]

            # Verify data can be queried from extended bundle
            results = await c_extended.to_dict()
            assert "country" in results
            assert len(results["country"]) == 1000

            # Verify new operations: url(), history()
            # URL is returned as a file:// URL
            assert temp2 in c_extended.url

            # Verify extended bundle has its own history
            history = c_extended.history()
            assert len(history) >= 1


@pytest.mark.asyncio
async def test_extend_bundle_with_operations():
    """Test extending a bundle and then applying operations"""
    import tempfile

    with tempfile.TemporaryDirectory() as temp1:
        with tempfile.TemporaryDirectory() as temp2:
            # Create and commit first bundle
            c1 = await bundlebase.create(temp1)
            c1 = await c1.attach(datafile("userdata.parquet"))
            await c1.commit("Initial commit")

            # Open and extend with chained operations
            c_opened = await bundlebase.open(temp1)
            c_extended = await (c_opened.extend(temp2)
                                .remove_column("email")
                                .filter("salary > $1", [50000.0]))

            # Verify the extended bundle has the transformations
            schema_ext = await c_extended.schema()
            field_names = [f.name for f in schema_ext.fields]
            assert "email" not in field_names
            assert "country" in field_names

            # Verify data was filtered
            results = await c_extended.to_dict()
            assert len(results["id"]) == 798  # 798 rows with salary > 50000

            # Verify operations are in history
            history = c_extended.history()
            assert len(history) >= 1
            # The first commit should have at least the attach operation
            first_commit = history[0]
            assert len(first_commit.operations) >= 1
            # Check operations have proper type and description
            for op in first_commit.operations:
                assert op.op_type is not None


@pytest.mark.asyncio
async def test_extend_bundle_multiple_operations():
    """Test extending a bundle and chaining multiple operations"""
    import tempfile

    with tempfile.TemporaryDirectory() as temp1:
        with tempfile.TemporaryDirectory() as temp2:
            # Create and commit first bundle
            c1 = await bundlebase.create(temp1)
            c1 = await c1.attach(datafile("userdata.parquet"))
            await c1.commit("Initial commit")

            # Open and extend with multiple chained operations
            c_opened = await bundlebase.open(temp1)
            c_extended = await (c_opened.extend(temp2)
                                .filter("salary > $1", [50000.0])
                                .rename_column("first_name", "fname"))

            # Verify data
            results = await c_extended.to_dict()
            returned_keys = list(results.keys())
            assert "id" in returned_keys
            assert "fname" in returned_keys
            assert "salary" in returned_keys
            assert "first_name" not in returned_keys  # Should be renamed to fname

            # Verify filter was applied
            assert len(results["id"]) == 798


@pytest.mark.asyncio
async def test_extend_bundle_conversion():
    """Test extending a bundle and converting to different formats"""
    import tempfile

    with tempfile.TemporaryDirectory() as temp1:
        with tempfile.TemporaryDirectory() as temp2:
            # Create and commit first bundle
            c1 = await bundlebase.create(temp1)
            c1 = await c1.attach(datafile("userdata.parquet"))
            await c1.commit("Initial commit")

            # Open and extend, then convert to various formats
            c_opened = await bundlebase.open(temp1)

            # Test to_pandas conversion
            df_pandas = await c_opened.extend(temp2).to_pandas()
            assert hasattr(df_pandas, "shape")
            assert df_pandas.shape[0] == 1000

            # Test to_dict conversion with operations
            c_opened2 = await bundlebase.open(temp1)

            # First, test url() on a simple extended bundle
            extended_simple = await c_opened2.extend(temp2)
            assert temp2 in extended_simple.url

            # Now test conversion with chained operations
            results = await c_opened.extend(temp2).filter("salary > $1", [50000.0]).to_dict()
            assert len(results["id"]) == 798


@pytest.mark.asyncio
async def test_extend_bundle_inherits_id():
    """Test that extended bundles inherit the same ID as the parent and 0000000.yaml is correct"""
    with tempfile.TemporaryDirectory() as temp1:
        with tempfile.TemporaryDirectory() as temp2:
            with tempfile.TemporaryDirectory() as temp3:
                # Create and commit first bundle
                c1 = await bundlebase.create(temp1)
                c1 = await c1.attach(datafile("userdata.parquet"))
                await c1.commit("Initial commit")

                # Read the ID from the base bundle's 00000000000000000.yaml (17 zeros)
                init_file_1 = os.path.join(temp1, "_bundlebase", "00000000000000000.yaml")
                with open(init_file_1, "r") as f:
                    init_data_1 = yaml.safe_load(f)

                base_id = init_data_1["id"]
                assert "id" in init_data_1, "Base bundle should have 'id' in InitCommit"
                assert "from" not in init_data_1, "Base bundle should NOT have 'from' in InitCommit"

                # Open and verify ID
                c1_opened = await bundlebase.open(temp1)
                assert c1_opened.id == base_id, "Opened bundle should have same ID as InitCommit"

                # Extend to second bundle
                c2 = await c1_opened.extend(temp2)
                c2 = await c2.remove_column("country")
                await c2.commit("Second commit")

                # Verify extended bundle's 00000000000000000.yaml has only 'from', not 'id'
                init_file_2 = os.path.join(temp2, "_bundlebase", "00000000000000000.yaml")
                with open(init_file_2, "r") as f:
                    init_data_2 = yaml.safe_load(f)

                assert "id" not in init_data_2, "Extended bundle should NOT have 'id' in InitCommit"
                assert "from" in init_data_2, "Extended bundle should have 'from' in InitCommit"
                assert temp1 in init_data_2["from"], "Extended bundle 'from' should point to parent"

                # Verify the opened extended bundle has the SAME id as the base bundle
                c2_opened = await bundlebase.open(temp2)
                assert c2_opened.id == base_id, "Extended bundle should inherit the same ID as base bundle"

                # Extend again to third bundle and verify ID is still the same
                c3 = await c2_opened.extend(temp3)
                c3 = await c3.remove_column("phone")
                await c3.commit("Third commit")

                # Verify third bundle's 00000000000000000.yaml
                init_file_3 = os.path.join(temp3, "_bundlebase", "00000000000000000.yaml")
                with open(init_file_3, "r") as f:
                    init_data_3 = yaml.safe_load(f)

                assert "id" not in init_data_3, "Third extended bundle should NOT have 'id' in InitCommit"
                assert "from" in init_data_3, "Third extended bundle should have 'from' in InitCommit"
                assert temp2 in init_data_3["from"], "Third bundle 'from' should point to second bundle"

                # Verify all bundles in the chain have the same ID
                c3_opened = await bundlebase.open(temp3)
                assert c3_opened.id == base_id, "Third extended bundle should still have the same ID as base bundle"


@pytest.mark.asyncio
async def test_create_index():
    """Test creating an index on a column"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Create an index on the id column
    c = await c.create_index("id")

    # Verify bundle still works
    assert await c.num_rows() == 1000
    results = await c.to_polars()
    assert len(results) == 1000


@pytest.mark.asyncio
async def test_rebuild_index():
    """Test rebuilding an index"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Create and rebuild an index
    c = await c.create_index("id")
    c = await c.rebuild_index("id")

    # Verify bundle still works
    assert await c.num_rows() == 1000
    results = await c.to_polars()
    assert len(results) == 1000


@pytest.mark.asyncio
async def test_multiple_indexes():
    """Test creating indexes on multiple columns"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Create indexes on multiple columns
    c = await c.create_index("id").create_index("salary")

    # Verify bundle still works
    assert await c.num_rows() == 1000
    results = await c.to_polars()
    assert len(results) == 1000


@pytest.mark.asyncio
async def test_index_with_operations():
    """Test indexing with other operations chained"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Create index and apply filter
    c = await c.create_index("salary").filter("salary > $1", [50000.0])

    # Verify filtering still works
    results = await c.to_polars()
    assert len(results) == 798
    assert all(results["salary"] > 50000.0)


@pytest.mark.asyncio
async def test_index_chaining():
    """Test fluent chaining with index operations"""
    c = await bundlebase.create(random_bundle())

    # Test chaining multiple index operations
    c = await (c.attach(datafile("userdata.parquet"))
               .create_index("id")
               .create_index("salary"))

    assert await c.num_rows() == 1000
    results = await c.to_polars()
    assert len(results) == 1000


@pytest.mark.asyncio
async def test_index_numeric_columns():
    """Test indexing on numeric columns"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Create indexes on numeric columns
    c = await c.create_index("id").create_index("salary")

    assert await c.num_rows() == 1000
    results = await c.to_polars()
    assert len(results) == 1000


@pytest.mark.asyncio
async def test_index_string_columns():
    """Test indexing on numeric columns (Utf8View not yet supported)"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Create indexes on numeric columns (string columns use Utf8View which is not yet supported)
    c = await c.create_index("id").create_index("salary")

    assert await c.num_rows() == 1000
    results = await c.to_polars()
    assert len(results) == 1000


@pytest.mark.asyncio
async def test_index_operations_with_commit():
    """Test that indexes persist across commit/open cycles"""
    with tempfile.TemporaryDirectory() as tmpdir:
        temp_path = f"{tmpdir}/indexed_bundle"

        # Create, attach, index, and commit
        c = await bundlebase.create(temp_path)
        c = await c.attach(datafile("userdata.parquet"))
        c = await c.create_index("id")
        await c.commit("Added index on id column")

        # Verify original bundle
        assert await c.num_rows() == 1000

        # Open and verify index operations
        c_opened = await bundlebase.open(temp_path)
        assert await c_opened.num_rows() == 1000
        results = await c_opened.to_polars()
        assert len(results) == 1000


@pytest.mark.asyncio
async def test_drop_index():
    """Test dropping an index"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Create an index on the id column
    c = await c.create_index("id")

    # Drop the index
    c = await c.drop_index("id")

    # Verify bundle still works
    assert await c.num_rows() == 1000
    results = await c.to_polars()
    assert len(results) == 1000

    # Should be able to recreate the index after dropping
    c = await c.create_index("id")
    assert await c.num_rows() == 1000


@pytest.mark.asyncio
async def test_drop_index_nonexistent():
    """Test dropping a non-existent index raises an error"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    # Try to drop an index that doesn't exist
    with pytest.raises(ValueError, match="No index found for column 'nonexistent'"):
        await c.drop_index("nonexistent")


@pytest.mark.asyncio
async def test_status_empty_bundle():
    """Test status() on a newly created bundle"""
    c = await bundlebase.create(random_bundle())

    # Empty bundle should have no operations
    status = c.status()
    assert isinstance(status, bundlebase.PyBundleStatus)
    assert status.is_empty()
    assert len(status.changes) == 0
    assert status.total_operations == 0


@pytest.mark.asyncio
async def test_status_single_operation():
    """Test status() after a single operation"""
    c = await bundlebase.create(random_bundle())
    c = await c.set_name("Test Bundle")

    # Should have one change
    status = c.status()
    assert isinstance(status, bundlebase.PyBundleStatus)
    assert len(status.changes) == 1
    assert status.total_operations == 1
    assert not status.is_empty()

    # Check change attributes
    change = status.changes[0]
    assert isinstance(change, bundlebase.PyChange)
    assert isinstance(change.id, str)
    assert len(change.id) > 0
    assert change.description == "Set name to Test Bundle"
    assert change.operation_count == 1


@pytest.mark.asyncio
async def test_status_multiple_operations():
    """Test status() with multiple changes"""
    c = await bundlebase.create(random_bundle())

    # Apply multiple operations
    c = await c.set_name("Test Bundle")
    c = await c.set_description("A test description")

    # Should have two changes
    status = c.status()
    assert isinstance(status, bundlebase.PyBundleStatus)
    assert len(status.changes) == 2
    assert status.total_operations == 2

    # Check first operation
    assert status.changes[0].description == "Set name to Test Bundle"
    assert status.changes[0].operation_count == 1

    # Check second operation
    assert status.changes[1].description == "Set description to A test description"
    assert status.changes[1].operation_count == 1


@pytest.mark.asyncio
async def test_status_with_data_operations():
    """Test status() with data transformation operations"""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("userdata.parquet"))

    status = c.status()
    assert isinstance(status, bundlebase.PyBundleStatus)
    assert len(status.changes) == 1
    assert "Attach" in status.changes[0].description or "attach" in status.changes[0].description.lower()


@pytest.mark.asyncio
async def test_status_chained_operations():
    """Test status() with chained operations"""
    c = await bundlebase.create(random_bundle())
    c = await (c.attach(datafile("userdata.parquet"))
               .set_name("User Data")
               .filter("salary > $1", [50000.0]))

    # Should have multiple changes
    status = c.status()
    assert isinstance(status, bundlebase.PyBundleStatus)
    assert len(status.changes) >= 2  # attach, set_name, filter

    # Verify all changes have proper attributes
    for change in status.changes:
        assert isinstance(change.id, str)
        assert isinstance(change.description, str)
        assert isinstance(change.operation_count, int)
        assert change.operation_count > 0


@pytest.mark.asyncio
async def test_status_after_commit():
    """Test status() is cleared after commit"""
    import tempfile

    with tempfile.TemporaryDirectory() as tmpdir:
        temp_path = f"{tmpdir}/status_test"

        c = await bundlebase.create(temp_path)
        c = await c.set_name("Test")

        # Should have operations before commit
        status_before = c.status()
        assert isinstance(status_before, bundlebase.PyBundleStatus)
        assert not status_before.is_empty()
        assert len(status_before.changes) > 0

        # Commit the operations
        await c.commit("Initial setup")

        # After commit, status should be cleared
        status_after = c.status()
        assert isinstance(status_after, bundlebase.PyBundleStatus)
        assert status_after.is_empty()
        assert len(status_after.changes) == 0


# ============================================================================
# Views Tests
# ============================================================================


@pytest.mark.asyncio
async def test_create_view_basic():
    """Test creating and opening a basic view."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create view with select
    adults = await c.select("select * where \"Index\" > 50")
    c = await c.create_view("high_index", adults)
    await c.commit("Add high_index view")

    # Open view
    view = await c.view("high_index")
    assert view is not None

    # Verify view has operations
    operations = view.operations()
    assert len(operations) >= 4  # CREATE PACK, ATTACH, CREATE VIEW, SELECT


@pytest.mark.asyncio
async def test_view_not_found():
    """Test error when opening non-existent view."""
    c = await bundlebase.create(random_bundle())

    # Try to open non-existent view
    with pytest.raises(Exception) as exc_info:
        await c.view("nonexistent")

    assert "View 'nonexistent' not found" in str(exc_info.value)


@pytest.mark.asyncio
async def test_view_inherits_parent_changes():
    """Test that views automatically see new parent commits."""
    container_url = random_bundle()
    c = await bundlebase.create(container_url)
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("v1")

    # Create view
    active = await c.select("select * where \"Index\" > 50")
    c = await c.create_view("active", active)
    await c.commit("v2")

    # Record initial view operations count
    initial_view = await c.view("active")
    initial_ops_count = len(initial_view.operations())

    # Reopen container and add more data to parent
    c_bundle = await bundlebase.open(container_url)
    c_reopened = c_bundle.extend(container_url)
    c_reopened = await c_reopened.attach(datafile("customers-101-150.csv"))
    await c_reopened.commit("v3 - more data")

    # View should see new parent commits through FROM chain
    view_after_parent_change = await c_reopened.view("active")
    new_ops_count = len(view_after_parent_change.operations())

    # The view should have more operations now
    assert new_ops_count > initial_ops_count, "View should inherit parent's new operations"


@pytest.mark.asyncio
async def test_view_with_multiple_operations():
    """Test view with multiple chained operations."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create view with multiple operations (select + filter)
    filtered = await c.select("select * where \"Index\" > 20")
    filtered = await filtered.filter("\"Index\" < 80")

    c = await c.create_view("mid_range", filtered)
    await c.commit("Add mid_range view")

    # Open view and verify it has the operations
    view = await c.view("mid_range")
    operations = view.operations()

    # Should have at least the select and filter operations from the view
    op_descriptions = [op.describe for op in operations]
    has_select = any("select" in desc.lower() for desc in op_descriptions)
    has_filter = any("FILTER" in desc for desc in op_descriptions)

    assert has_select, "View should have select operation"
    assert has_filter, "View should have filter operation"


@pytest.mark.asyncio
async def test_duplicate_view_name():
    """Test error when creating duplicate view names."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial")

    # Create first view
    adults1 = await c.select("select * where \"Index\" > 50")
    c = await c.create_view("adults", adults1)
    await c.commit("Add first adults view")

    # Try to create view with same name
    adults2 = await c.select("select * where \"Index\" > 70")
    with pytest.raises(Exception) as exc_info:
        await c.create_view("adults", adults2)

    assert "View 'adults' already exists" in str(exc_info.value)


@pytest.mark.asyncio
async def test_view_dataframe_execution():
    """Test that views can execute dataframe queries."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create view with country filter
    chile = await c.select("select * from data where Country = 'Chile'")
    c = await c.create_view("chile", chile)
    await c.commit("Add chile view")

    # Open view and execute dataframe query
    view = await c.view("chile")

    # This should work if data is inherited correctly
    schema = await view.schema()
    assert len(schema) > 0, "View should have schema"

    # Verify Country field exists
    country_field = schema.field("Country")
    assert country_field is not None, "View should have 'Country' column"
    assert country_field.name == "Country"


@pytest.mark.asyncio
async def test_view_to_polars():
    """Test converting view results to Polars DataFrame."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create view with a simple filter
    high_idx = await c.select("select * from data where \"Index\" > 50")
    c = await c.create_view("high_index_polars", high_idx)
    await c.commit("Add high_index_polars view")

    # Open view and convert to Polars
    view = await c.view("high_index_polars")
    df = await view.to_polars()

    assert isinstance(df, polars.DataFrame), "Should return Polars DataFrame"
    assert len(df) > 0, "Should have some high index customers"

    # Verify all rows have Index > 50
    assert all(df["Index"] > 50), "All rows should have Index > 50"


@pytest.mark.asyncio
async def test_view_to_pandas():
    """Test converting view results to Pandas DataFrame."""
    import pandas as pd

    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create view for high index values
    high_idx = await c.select("select * from data where \"Index\" > 80")
    c = await c.create_view("high_index", high_idx)
    await c.commit("Add high_index view")

    # Open view and convert to Pandas
    view = await c.view("high_index")
    df = await view.to_pandas()

    assert isinstance(df, pd.DataFrame), "Should return Pandas DataFrame"
    assert len(df) > 0, "Should have some high index customers"
    assert all(df["Index"] > 80), "All rows should have Index > 80"


@pytest.mark.asyncio
async def test_view_chaining():
    """Test that you can create multiple views from the same container."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create first view
    view1 = await c.select("select * where \"Index\" > 20")
    c = await c.create_view("view1", view1)
    await c.commit("Add first view")

    # Create second view from base container
    view2 = await c.select("select * where \"Index\" < 80")
    c = await c.create_view("view2", view2)
    await c.commit("Add second view")

    # Both views should be accessible
    v1 = await c.view("view1")
    v2 = await c.view("view2")

    assert v1 is not None
    assert v2 is not None

    # Both should have operations
    assert len(v1.operations()) >= 4
    assert len(v2.operations()) >= 4


@pytest.mark.asyncio
async def test_views_method():
    """Test the views() method returns id->name mapping."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create multiple views
    view1 = await c.select("select * where \"Index\" > 50")
    c = await c.create_view("high_index", view1)

    view2 = await c.select("select * where \"Index\" < 30")
    c = await c.create_view("low_index", view2)

    await c.commit("Add views")

    # Get views map (id->name)
    views_map = c.views()

    assert isinstance(views_map, dict), "Should return a dictionary"
    assert len(views_map) == 2, "Should have 2 views"

    # Check that both view names are in the values
    view_names = list(views_map.values())
    assert "high_index" in view_names
    assert "low_index" in view_names


@pytest.mark.asyncio
async def test_view_lookup_by_name_and_id():
    """Test that view() can accept either a name or an ID."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create a view
    high_index = await c.select("select * where \"Index\" > 50")
    c = await c.create_view("high_index", high_index)
    await c.commit("Add view")

    # Get the view ID
    views_map = c.views()
    assert len(views_map) == 1, "Should have 1 view"
    view_id = list(views_map.keys())[0]
    view_name = views_map[view_id]
    assert view_name == "high_index"

    # Test 1: Open view by name
    view_by_name = await c.view("high_index")
    assert view_by_name is not None, "Should open view by name"
    assert view_by_name.url is not None, "View should have a URL"

    # Test 2: Open view by ID
    view_by_id = await c.view(view_id)
    assert view_by_id is not None, "Should open view by ID"
    assert view_by_id.url is not None, "View should have a URL"

    # Test 3: Both views should point to the same location
    assert view_by_name.url == view_by_id.url, \
        "View opened by name and ID should have same URL"

    # Test 4: Non-existent name should error with helpful message
    with pytest.raises(Exception) as exc_info:
        await c.view("nonexistent")
    err_msg = str(exc_info.value)
    assert "View 'nonexistent' not found" in err_msg, "Error should mention view not found"
    assert "high_index" in err_msg, "Error should list available views"
    assert view_id in err_msg, "Error should include view ID"

    # Test 5: Non-existent ID should error
    with pytest.raises(Exception) as exc_info:
        await c.view("ff")
    err_msg = str(exc_info.value)
    assert "View with ID" in err_msg, "Error should mention ID not found"


@pytest.mark.asyncio
async def test_rename_view_basic():
    """Test basic rename_view functionality."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create a view
    adults = await c.select("select * from data where \"Index\" > 21")
    c = await c.create_view("adults", adults)
    await c.commit("Add adults view")

    # Rename the view
    c = await c.rename_view("adults", "adults_view")
    await c.commit("Renamed view")

    # Verify old name doesn't work
    with pytest.raises(Exception) as exc_info:
        await c.view("adults")
    assert "not found" in str(exc_info.value)

    # Verify new name works
    view = await c.view("adults_view")
    assert view is not None

    # Verify views() returns new name
    views_map = c.views()
    assert len(views_map) == 1
    view_name = list(views_map.values())[0]
    assert view_name == "adults_view"


@pytest.mark.asyncio
async def test_rename_view_old_name_not_found():
    """Test error when trying to rename non-existent view."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Try to rename non-existent view
    with pytest.raises(Exception) as exc_info:
        await c.rename_view("nonexistent", "new_name")
    err_msg = str(exc_info.value)
    assert "View 'nonexistent' not found" in err_msg


@pytest.mark.asyncio
async def test_rename_view_new_name_exists():
    """Test error when renaming to an existing view name."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create two views
    view1 = await c.select("select * from data where \"Index\" > 21")
    c = await c.create_view("view1", view1)

    view2 = await c.select("select * from data where \"Index\" < 30")
    c = await c.create_view("view2", view2)
    await c.commit("Add two views")

    # Try to rename view1 to view2 (conflict)
    with pytest.raises(Exception) as exc_info:
        await c.rename_view("view1", "view2")
    err_msg = str(exc_info.value)
    assert "already exists" in err_msg


@pytest.mark.asyncio
async def test_rename_view_preserves_view_data():
    """Test that renaming a view preserves its data."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create a view
    high_index = await c.select("select * from data where \"Index\" > 50")
    c = await c.create_view("high_index", high_index)
    await c.commit("Add view")

    # Get data before rename
    view_before = await c.view("high_index")
    df_before = await view_before.to_pandas()
    rows_before = len(df_before)

    # Rename the view
    c = await c.rename_view("high_index", "high_values")
    await c.commit("Renamed view")

    # Get data after rename
    view_after = await c.view("high_values")
    df_after = await view_after.to_pandas()
    rows_after = len(df_after)

    assert rows_before == rows_after, "View should have same row count after rename"


@pytest.mark.asyncio
async def test_rename_view_commit_and_reopen():
    """Test that renamed views persist after commit and reopen."""
    bundle_url = random_bundle()
    c = await bundlebase.create(bundle_url)
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create and rename a view
    adults = await c.select("select * from data where \"Index\" > 21")
    c = await c.create_view("adults", adults)
    await c.commit("Add adults view")

    c = await c.rename_view("adults", "adults_renamed")
    await c.commit("Renamed view")

    # Reopen the bundle
    bundle = await bundlebase.open(bundle_url)

    # Verify old name doesn't exist
    with pytest.raises(Exception):
        await bundle.view("adults")

    # Verify new name works
    view = await bundle.view("adults_renamed")
    assert view is not None

    # Verify views() shows correct name
    views_map = bundle.views()
    assert len(views_map) == 1
    view_name = list(views_map.values())[0]
    assert view_name == "adults_renamed"


@pytest.mark.asyncio
async def test_drop_view_basic():
    """Test basic drop_view functionality."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create a view
    adults = await c.select("select * from data where \"Index\" > 21")
    c = await c.create_view("adults", adults)
    await c.commit("Add adults view")

    # Verify view exists
    view = await c.view("adults")
    assert view is not None
    assert len(c.views()) == 1

    # Drop the view
    c = await c.drop_view("adults")
    await c.commit("Dropped view")

    # Verify view no longer exists
    with pytest.raises(Exception) as exc_info:
        await c.view("adults")
    assert "not found" in str(exc_info.value)

    # Verify views map is empty
    views_map = c.views()
    assert len(views_map) == 0


@pytest.mark.asyncio
async def test_drop_view_not_found():
    """Test error when trying to drop non-existent view."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Try to drop non-existent view
    with pytest.raises(Exception) as exc_info:
        await c.drop_view("nonexistent")
    err_msg = str(exc_info.value)
    assert "View 'nonexistent' not found" in err_msg


@pytest.mark.asyncio
async def test_drop_view_commit_and_reopen():
    """Test that dropped views persist after commit and reopen."""
    bundle_url = random_bundle()
    c = await bundlebase.create(bundle_url)
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create and drop a view
    adults = await c.select("select * from data where \"Index\" > 21")
    c = await c.create_view("adults", adults)
    await c.commit("Add adults view")

    c = await c.drop_view("adults")
    await c.commit("Dropped view")

    # Reopen the bundle
    bundle = await bundlebase.open(bundle_url)

    # Verify view doesn't exist
    with pytest.raises(Exception):
        await bundle.view("adults")

    # Verify views map is empty
    views_map = bundle.views()
    assert len(views_map) == 0


@pytest.mark.asyncio
async def test_drop_view_preserves_other_views():
    """Test that dropping one view doesn't affect other views."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create two views
    view1 = await c.select("select * from data where \"Index\" > 21")
    c = await c.create_view("view1", view1)

    view2 = await c.select("select * from data where \"Index\" < 30")
    c = await c.create_view("view2", view2)
    await c.commit("Add two views")

    # Verify both views exist
    assert len(c.views()) == 2

    # Drop one view
    c = await c.drop_view("view1")
    await c.commit("Dropped view1")

    # Verify view1 is gone
    with pytest.raises(Exception):
        await c.view("view1")

    # Verify view2 still exists
    view2_after = await c.view("view2")
    assert view2_after is not None

    # Verify views map only contains view2
    views_map = c.views()
    assert len(views_map) == 1
    view_name = list(views_map.values())[0]
    assert view_name == "view2"


@pytest.mark.asyncio
async def test_drop_view_twice_fails():
    """Test error when trying to drop the same view twice."""
    c = await bundlebase.create(random_bundle())
    c = await c.attach(datafile("customers-0-100.csv"))
    await c.commit("Initial data")

    # Create a view
    adults = await c.select("select * from data where \"Index\" > 21")
    c = await c.create_view("adults", adults)
    await c.commit("Add adults view")

    # Drop the view
    c = await c.drop_view("adults")
    await c.commit("Dropped view")

    # Try to drop it again
    with pytest.raises(Exception) as exc_info:
        await c.drop_view("adults")
    err_msg = str(exc_info.value)
    assert "View 'adults' not found" in err_msg
