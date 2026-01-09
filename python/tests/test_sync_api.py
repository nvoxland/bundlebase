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
        assert c.schema.is_empty()
        assert len(c.schema) == 0

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

    def test_sync_remove_column(self):
        """Test removing a column synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.remove_column("country")

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
        c.filter("salary > $1", [50000.0])

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
        """Test chaining attach and remove_column without await."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet")).remove_column("country")

        field_names = [f.name for f in c.schema.fields]
        assert "country" not in field_names

    def test_chain_multiple_operations(self):
        """Test chaining multiple operations."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet")).remove_column("country").rename_column(
            "first_name", "fname"
        ).filter("salary > $1", [50000.0])

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
            .filter("salary > $1", [50000.0])
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
        assert c.num_rows() == 0

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
        c.attach(datafile("userdata.parquet")).filter("salary > $1", [50000.0])

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
        c.create_index("id")

        # Verify bundle still works
        assert c.num_rows() == 1000

    def test_sync_rebuild_index(self):
        """Test rebuilding an index synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.create_index("id")
        c.rebuild_index("id")

        assert c.num_rows() == 1000

    def test_sync_multiple_indexes(self):
        """Test creating multiple indexes."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.create_index("id").create_index("salary")

        assert c.num_rows() == 1000

    def test_sync_drop_index(self):
        """Test dropping an index synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.create_index("id")
        c.drop_index("id")

        # Verify bundle still works
        assert c.num_rows() == 1000

        # Should be able to recreate the index after dropping
        c.create_index("id")
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
                c_extended = c_opened.extend(temp2)

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
                c_extended = c_opened.extend(temp2).filter("salary > $1", [50000.0])

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
        assert isinstance(c.version, str)
        assert len(c.version) == 12  # 12-char hex

    def test_schema_property(self):
        """Test schema property."""
        c = bb.create(random_bundle())
        assert c.schema.is_empty()

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
            datafile("sales-regions.csv"),
            '$base."Country" = regions."Country"',
        )

        results = c.to_dict()
        assert "Country" in results

        # Then attach additional data to the existing join
        c.attach_to_join("regions", datafile("sales-regions.csv"))

        results = c.to_dict()
        assert "Country" in results


class TestSyncSelect:
    """Test synchronous selecct operations."""

    def test_sync_select(self):
        """Test SQL query execution synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))

        # select() returns a new forked bundle
        c2 = c.select("SELECT * FROM bundle LIMIT 10")

        # Original bundle should be unchanged
        results_original = c.to_dict()
        assert len(results_original["id"]) == 1000

        # Forked bundle should have the query applied
        results_selected = c2.to_dict()
        assert len(results_selected["id"]) == 10

    def test_sync_explain(self):
        """Test query explanation synchronously."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.filter("salary > $1", [50000.0])

        plan = c.explain()
        assert isinstance(plan, str)
        assert len(plan) > 0


class TestSyncCreateView:
    """Test synchronous create_view operations."""

    def test_sync_create_view_basic(self):
        """Test creating a view synchronously (no deadlock)."""
        c = bb.create(random_bundle())
        c.attach(datafile("customers-0-100.csv"))

        # select() returns a new forked bundle (not the same object as c)
        # This used to cause a deadlock in the Python bindings
        filtered = c.select("select * from data limit 10")
        c.create_view("limited", filtered)

        # If we get here, create_view completed successfully (no deadlock)
        assert True

    def test_sync_create_view_with_commit(self):
        """Test creating a view and committing synchronously."""
        with tempfile.TemporaryDirectory() as tmpdir:
            c = bb.create(tmpdir)
            c.attach(datafile("customers-0-100.csv"))

            # Create view from select
            filtered = c.select("select * from data limit 10")
            c.create_view("limited", filtered)
            c.commit("Added limited view")

            # If we get here without deadlock, test passes
            assert True

    def test_sync_create_view_chaining(self):
        """Test chaining operations with create_view."""
        c = bb.create(random_bundle())
        c.attach(datafile("customers-0-100.csv"))

        filtered = c.select("select * from data limit 10")
        c.create_view("limited", filtered).set_name("Customer Data")

        assert c.name == "Customer Data"

    def test_sync_create_view_no_double_commit(self):
        """Verify select operation is not committed to main container."""
        with tempfile.TemporaryDirectory() as tmpdir:
            c = bb.create(tmpdir)
            c.attach(datafile("customers-0-100.csv"))
            c.commit("Initial data")

            # Check status after first commit
            assert c.status().is_empty(), "Should have no uncommitted changes after commit"

            # select returns a new forked bundle
            rs = c.select("select * from data limit 10")

            # c should still have no uncommitted changes (select created a fork)
            assert c.status().is_empty()

            # rs should have uncommitted select operation
            assert not rs.status().is_empty()

            # Create view from the forked bundle
            c.create_view("limited", rs)

            # Now c should have uncommitted create_view operation
            assert len(c.status().changes) == 1

            c.commit("Added view")

            # Reopen and check commit history
            c2 = bb.open(tmpdir)
            history = c2.history()

            # Most recent commit should only have CreateViewOp, not SelectOp
            assert len(history) == 2
            assert history[-1].message == "Added view"
            assert "Create view 'limited'" in history[-1].changes[0].description


class TestSyncStatus:
    """Test synchronous status() operations."""

    def test_sync_status_empty(self):
        """Test status() on empty bundle."""
        c = bb.create(random_bundle())

        status = c.status()
        assert hasattr(status, 'is_empty')
        assert status.is_empty()
        assert len(status.changes) == 0

    def test_sync_status_single_operation(self):
        """Test status() after single operation."""
        c = bb.create(random_bundle())
        c.set_name("Test Bundle")

        status = c.status()
        assert len(status.changes) == 1
        assert status.total_operations == 1

        change = status.changes[0]
        assert isinstance(change.id, str)
        assert len(change.id) > 0
        assert change.description == "Set name to Test Bundle"
        assert change.operation_count == 1

    def test_sync_status_multiple_operations(self):
        """Test status() with multiple operations."""
        c = bb.create(random_bundle())
        c.set_name("Test Bundle")
        c.set_description("A test description")

        status = c.status()
        assert len(status.changes) == 2
        assert status.changes[0].description == "Set name to Test Bundle"
        assert status.changes[1].description == "Set description to A test description"

    def test_sync_status_chained_operations(self):
        """Test status() with chained operations."""
        c = bb.create(random_bundle())
        c.attach(datafile("userdata.parquet"))
        c.set_name("User Data")
        c.filter("salary > $1", [50000.0])

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
        with tempfile.TemporaryDirectory() as tmpdir:
            temp_path = f"{tmpdir}/status_test"

            c = bb.create(temp_path)
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
