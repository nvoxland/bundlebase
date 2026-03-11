"""Tests for the synchronous Bundlebase API.

Tests the bundlebase.sync module to ensure all operations work correctly
without async/await syntax.
"""

import tempfile

import bundlebase.sync as bb
from conftest import datafile, random_bundle


class TestSyncCreate:
    """Test synchronous bundle creation."""

    def test_sync_create_empty(self):
        """Test creating an empty bundle synchronously."""
        c = bb.create(random_bundle())
        assert c is not None
        # Base pack is auto-created without an operation, so status should be empty
        status = c.status()
        assert len(status.changes) == 0
        assert status.is_empty()

    def test_sync_create_with_path(self):
        """Test creating bundle with specific path."""
        with tempfile.TemporaryDirectory() as tmpdir:
            c = bb.create(tmpdir)
            assert c is not None
            assert c.url is not None


# @pytest.mark.skip(reason="Causes tests to hang")
# class TestSyncAttach:
#     """Test synchronous attach operations."""
#
#     @pytest.mark.skip(reason="Causes tests to hang")
#     def test_sync_attach_parquet(self):
#         """Test attaching parquet file without await."""
#         c = bb.create(random_bundle())
#         c.attach(datafile("userdata.parquet"))
#
#         # Verify attachment worked
#         assert len(c.schema) == 13
#         assert c.num_rows() == 1000

    # @pytest.mark.skip(reason="Causes tests to hang")
    # def test_sync_attach_csv(self):
    #     """Test attaching CSV file synchronously."""
    #     c = bb.create(random_bundle())
    #     c.attach(datafile("customers-0-100.csv"))
    #
    #     assert len(c.schema) == 12
    #     assert c.num_rows() == 100
    #
    # @pytest.mark.skip(reason="Causes tests to hang")
    # def test_sync_attach_json(self):
    #     """Test attaching JSON file synchronously."""
    #     c = bb.create(random_bundle())
    #     c.attach(datafile("objects.json"))
    #
    #     assert len(c.schema) == 4
    #     assert c.num_rows() == 4


class TestSyncOperations:
    """Test synchronous mutation operations."""

    def test_sync_drop_column(self):
        """Test removing a column synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.drop_column("country")

        field_names = [f.name for f in c.schema.fields]
        assert "country" not in field_names
        assert "id" in field_names

    def test_sync_rename_column(self):
        """Test renaming a column synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.rename_column("first_name", "fname")

        field_names = [f.name for f in c.schema.fields]
        assert "fname" in field_names
        assert "first_name" not in field_names

    def test_sync_filter(self):
        """Test filtering rows synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])

        results = c.to_dict()
        assert len(results["id"]) == 798  # 798 rows with salary > 50000

    def test_sync_set_name(self):
        """Test setting bundle name synchronously."""
        c = bb.create(random_bundle())
        assert c.name is None

        c.set_name("My Bundle")
        assert c.name == "My Bundle"

    def test_sync_set_description(self):
        """Test setting bundle description synchronously."""
        c = bb.create(random_bundle())
        assert c.description is None

        c.set_description("Test description")
        assert c.description == "Test description"


class TestSyncChaining:
    """Test fluent method chaining in synchronous mode."""

    def test_chain_attach_and_remove(self):
        """Test chaining attach and drop_column without await."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet")).drop_column("country")

        field_names = [f.name for f in c.schema.fields]
        assert "country" not in field_names

    def test_chain_multiple_operations(self):
        """Test chaining multiple operations."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet")).drop_column("country").rename_column(
            "first_name", "fname"
        ).filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])

        results = c.to_dict()
        assert "fname" in results
        assert "first_name" not in results
        assert "country" not in results
        assert len(results["id"]) == 798

    def test_chain_with_conversion(self):
        """Test chaining operations ending with conversion."""
        df = (
            bb.create(random_bundle())
            .attach(datafile("userdata.parquet"))
            .filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])
            .to_pandas()
        )

        assert len(df) == 798
        assert "id" in df.columns


class TestSyncConversions:
    """Test synchronous data conversions."""

    def test_sync_to_pandas(self):
        """Test conversion to pandas without await."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))

        df = c.to_pandas()
        assert df.shape[0] == 1000
        assert "id" in df.columns
        assert "first_name" in df.columns

    def test_sync_to_polars(self):
        """Test conversion to polars without await."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))

        df = c.to_polars()
        assert df.shape[0] == 1000
        assert "id" in df.columns

    def test_sync_to_dict(self):
        """Test conversion to dict without await."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))

        data = c.to_dict()
        assert isinstance(data, dict)
        assert "id" in data
        assert len(data["id"]) == 1000

    def test_sync_to_numpy(self):
        """Test conversion to numpy arrays without await."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))

        arrays = c.to_numpy()
        assert isinstance(arrays, dict)
        assert "id" in arrays
        assert len(arrays["id"]) == 1000

    def test_sync_num_rows(self):
        """Test getting row count without await."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        assert c.num_rows() == 1000


class TestSyncStreaming:
    """Test synchronous streaming operations."""

    def test_stream_batches(self):
        """Test streaming batches synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))

        total_rows = 0
        batch_count = 0
        for batch in bb.stream_batches(c):
            total_rows += batch.num_rows
            batch_count += 1

        assert total_rows == 1000
        assert batch_count > 0

    def test_stream_filtered_data(self):
        """Test streaming filtered data."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet")).filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])

        total_rows = 0
        for batch in bb.stream_batches(c):
            total_rows += batch.num_rows

        assert total_rows == 798


class TestSyncCommit:
    """Test synchronous commit operations."""

    def test_sync_commit(self):
        """Test commit without await."""
        with tempfile.TemporaryDirectory() as tmpdir:
            c = bb.create(tmpdir)
            c.attach(datafile("userdata.parquet"))
            c.commit("Initial commit")

            # Verify by reopening
            c2 = bb.open(tmpdir)
            assert c2.num_rows() == 1000

    def test_sync_open_saved(self):
        """Test opening a saved bundle synchronously."""
        with tempfile.TemporaryDirectory() as tmpdir:
            # Create and save
            c1 = bb.create(tmpdir)
            c1.attach(datafile("userdata.parquet"))
            c1.set_name("Test Bundle")
            c1.commit("Test commit")

            # Reopen synchronously
            c2 = bb.open(tmpdir)
            assert c2.num_rows() == 1000
            assert c2.name == "Test Bundle"


class TestSyncIndex:
    """Test synchronous index operations."""

    def test_sync_create_index(self):
        """Test creating an index synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.create_index("id", "column")

        # Verify bundle still works
        assert c.num_rows() == 1000

    def test_sync_rebuild_index(self):
        """Test rebuilding an index synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.create_index("id", "column")
        c.rebuild_index("id")

        assert c.num_rows() == 1000

    def test_sync_multiple_indexes(self):
        """Test creating multiple indexes."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.create_index("id", "column").create_index("salary", "column")

        assert c.num_rows() == 1000

    def test_sync_drop_index(self):
        """Test dropping an index synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.create_index("id", "column")
        c.drop_index("id")

        # Verify bundle still works
        assert c.num_rows() == 1000

        # Should be able to recreate the index after dropping
        c.create_index("id", "column")
        assert c.num_rows() == 1000


class TestSyncExtend:
    """Test synchronous extend operations."""

    def test_sync_extend_basic(self):
        """Test extending a bundle synchronously."""
        with tempfile.TemporaryDirectory() as temp1:
            with tempfile.TemporaryDirectory() as temp2:
                # Create and save first bundle
                c1 = bb.create(temp1)
                c1.attach(datafile("userdata.parquet"))
                c1.commit("Initial commit")

                # Open and extend
                c_opened = bb.open(temp1)
                c_extended = c_opened.extend(data_dir=temp2)

                # Verify extended bundle
                assert c_extended.num_rows() == 1000
                assert "country" in [f.name for f in c_extended.schema.fields]

    def test_sync_extend_with_operations(self):
        """Test extending and applying operations."""
        with tempfile.TemporaryDirectory() as temp1:
            with tempfile.TemporaryDirectory() as temp2:
                # Create and save
                c1 = bb.create(temp1)
                c1.attach(datafile("userdata.parquet"))
                c1.commit("Initial commit")

                # Extend and transform
                c_opened = bb.open(temp1)
                c_extended = c_opened.extend(data_dir=temp2).filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])

                results = c_extended.to_dict()
                assert len(results["id"]) == 798


class TestSyncProperties:
    """Test synchronous property access."""

    def test_properties(self):
        """Test property getters."""
        c = bb.create(random_bundle())
        c.set_name("Test")
        c.set_description("Test description")

        assert c.name == "Test"
        assert c.description == "Test description"
        assert c.version == "UNCOMMITTED"

    def test_schema_property(self):
        """Test schema property."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        assert not c.schema.is_empty()
        assert len(c.schema) == 13


class TestSyncJoin:
    """Test synchronous join operations."""

    def test_sync_join(self):
        """Test join operation synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("customers-0-100.csv"))
        c.join(
            "regions",
            'base."Country" = regions."Country"',
            datafile("sales-regions.csv"),
        )

        results = c.to_dict()
        assert "Country" in results

        # Then attach additional data to the existing join
        c.attach(datafile("sales-regions.csv"), pack="regions")

        results = c.to_dict()
        assert "Country" in results


class TestSyncQuery:
    """Test synchronous query operations."""

    def test_sync_query(self):
        """Test SQL query execution synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))

        # query() returns query results directly
        result = c.query("SELECT * FROM bundle LIMIT 10")

        # Original bundle should be unchanged
        results_original = c.to_dict()
        assert len(results_original["id"]) == 1000

        # Query result should have limited rows
        results_queried = result.to_dict()
        assert len(results_queried["id"]) == 10

    def test_sync_explain(self):
        """Test query explanation synchronously returns an ExplainResult."""
        from bundlebase.sync import ExplainResult

        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])

        result = c.explain()
        assert isinstance(result, ExplainResult)
        # ExplainResult should have a meaningful string representation
        plan_str = str(result)
        assert len(plan_str) > 0


class TestSyncCreateView:
    """Test synchronous create_view operations."""

    def test_sync_create_view_basic(self):
        """Test creating a view synchronously (no deadlock)."""
        c = bb.create(random_bundle())
        c.attach(datafile("customers-0-100.csv"))

        # create_view(name, sql) creates a view and returns its builder
        # This used to cause a deadlock in the Python bindings
        c.create_view("limited", "select * from bundle limit 10")

        # If we get here, create_view completed successfully (no deadlock)
        assert True

    def test_sync_create_view_with_commit(self):
        """Test creating a view and committing synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("customers-0-100.csv"))

        # Create view from SQL
        c.create_view("limited", "select * from bundle limit 10")
        c.commit("Added limited view")

        # If we get here without deadlock, test passes
        assert True

    def test_sync_create_view_chaining(self):
        """Test chaining operations with create_view."""
        c = bb.create(random_bundle())
        c.attach(datafile("customers-0-100.csv"))

        # create_view returns the view's builder, set_name on that
        view = c.create_view("limited", "select * from bundle limit 10")
        view.set_name("Limited View")

        assert view.name == "Limited View"

    def test_sync_create_view_no_double_commit(self):
        """Verify create_view SQL is stored in the view, not in main container."""
        with tempfile.TemporaryDirectory() as tmpdir:
            c = bb.create(tmpdir)
            c.attach(datafile("customers-0-100.csv"))
            c.commit("Initial data")

            # Check status after first commit
            assert c.status().is_empty(), "Should have no uncommitted changes after commit"

            # Create view with SQL - SQL is stored in the view, not as a separate operation
            c.create_view("limited", "select * from bundle limit 10")

            # Now c should have uncommitted create_view operation
            assert len(c.status().changes) == 1

            c.commit("Added view")

            # Reopen and check commit history
            c2 = bb.open(tmpdir)
            history = c2.history()

            # Most recent commit should only have CreateViewOp
            assert len(history) == 2
            assert history[-1].message == "Added view"
            assert "Create view 'limited'" in history[-1].changes[0].description


class TestSyncStatus:
    """Test synchronous status() operations."""

    def test_sync_status_empty(self):
        """Test status() on empty bundle."""
        c = bb.create(random_bundle())

        # Base pack is auto-created without an operation, so status should be empty
        status = c.status()
        assert hasattr(status, 'is_empty')
        assert status.is_empty()
        assert len(status.changes) == 0

    def test_sync_status_single_operation(self):
        """Test status() after single operation."""
        c = bb.create(random_bundle())
        c.set_name("Test Bundle")

        # Should have 1 change: set_name
        status = c.status()
        assert len(status.changes) == 1
        assert status.total_operations == 1

        # Check the set_name change
        change = status.changes[0]
        assert isinstance(change.id, str)
        assert len(change.id) > 0
        assert change.description == "SET NAME 'Test Bundle'"
        assert change.operation_count == 1

    def test_sync_status_multiple_operations(self):
        """Test status() with multiple operations."""
        c = bb.create(random_bundle())
        c.set_name("Test Bundle")
        c.set_description("A test description")

        # Should have 2 changes: set_name + set_description
        status = c.status()
        assert len(status.changes) == 2
        assert status.changes[0].description == "SET NAME 'Test Bundle'"
        assert status.changes[1].description == "SET DESCRIPTION 'A test description'"

    def test_sync_status_chained_operations(self):
        """Test status() with chained operations."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.set_name("User Data")
        c.filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])

        status = c.status()
        assert len(status.changes) >= 2

        # Verify all changes have proper attributes
        for change in status.changes:
            assert isinstance(change.id, str)
            assert isinstance(change.description, str)
            assert isinstance(change.operation_count, int)
            assert change.operation_count > 0

    def test_sync_status_after_commit(self):
        """Test status() is cleared after commit."""
        c = bb.create(random_bundle())
        c.set_name("Test")

        # Should have operations before commit
        status_before = c.status()
        assert not status_before.is_empty()
        assert len(status_before.changes) > 0

        # Commit the operations
        c.commit("Initial setup")

        # After commit, status should be cleared
        status_after = c.status()
        assert status_after.is_empty()
        assert len(status_after.changes) == 0


class TestSyncDetachBlock:
    """Test synchronous detach_block operations."""

    def test_sync_detach_block(self):
        """Test detaching a block synchronously."""
        location = datafile("customers-0-100.csv")
        c = bb.create(random_bundle())
        c.attach(location)

        # Verify the block is attached
        assert c.num_rows() == 100

        # Detach the block
        c.detach_block(location)

        # Verify the block is detached (no rows)
        assert c.num_rows() == 0


class TestSyncReplaceBlock:
    """Test synchronous replace_block operations."""

    def test_sync_replace_block(self):
        """Test replacing a block's location synchronously."""
        old_location = datafile("customers-0-100.csv")
        new_location = datafile("customers-101-150.csv")

        c = bb.create(random_bundle())
        c.attach(old_location)

        # Verify initial data
        assert c.num_rows() == 100

        # Replace with new location
        c.replace_block(old_location, new_location)

        # Verify the data comes from the new location (50 rows)
        assert c.num_rows() == 50


class TestSyncSource:
    """Test synchronous source operations."""

    def test_sync_create_source(self):
        """Test defining a source synchronously."""
        c = bb.create(random_bundle())
        c.create_source("remote_dir", {"url": "file:///some/path/"})
        assert c is not None

    def test_sync_create_source_chaining(self):
        """Test defining a source with chaining."""
        c = bb.create(random_bundle())
        c.set_name("Test Bundle").create_source(
            "remote_dir", {"url": "file:///data/", "patterns": "**/*.parquet"}
        )
        assert c.name == "Test Bundle"

    def test_sync_fetch(self):
        """Test fetch synchronously with empty source."""
        c = bb.create(random_bundle())
        # Define a source pointing to a non-existent location (no files to find)
        c.create_source("remote_dir", {"url": "file:///nonexistent/path/"})

        # fetch should return FetchResults with no changes
        results = c.fetch("base", "add")
        assert len(results) == 1
        assert results[0].total_count() == 0

    def test_sync_import_connector(self):
        """Test import_connector synchronously."""
        config = {"system": {"allow_external_code": "true"}}
        c = bb.create(random_bundle(), config=config)
        c.import_connector("test.my_source", "ipc", "/usr/bin/test")
        assert c is not None

    def test_sync_import_temp_connector(self):
        """Test import_temp_connector synchronously."""
        config = {"system": {"allow_external_code": "true"}}
        c = bb.create(random_bundle(), config=config)
        c.import_temp_connector("test.my_source", "python", "mod:Class")
        assert c is not None

    def test_sync_drop_connector(self):
        """Test drop_connector synchronously."""
        config = {"system": {"allow_external_code": "true"}}
        c = bb.create(random_bundle(), config=config)
        c.import_connector("test.my_source", "ipc", "/usr/bin/test")
        c.drop_connector("test.my_source")
        assert c is not None

    def test_sync_drop_connector_with_platform(self):
        """Test drop_connector with platform synchronously."""
        config = {"system": {"allow_external_code": "true"}}
        c = bb.create(random_bundle(), config=config)
        c.import_connector("test.my_source", "ipc", "/usr/bin/test", "linux/amd64")
        c.drop_connector("test.my_source", "linux/amd64")
        assert c is not None

    def test_sync_drop_temp_connector(self):
        """Test drop_temp_connector synchronously."""
        config = {"system": {"allow_external_code": "true"}}
        c = bb.create(random_bundle(), config=config)
        c.import_temp_connector("test.my_source", "python", "mod:Class")
        result = c.drop_temp_connector("test.my_source")
        assert "Dropped 1 temporary connector" in result
