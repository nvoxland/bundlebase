"""Tests for detecting unawaited async operations.

Tests the warning system that detects when async Bundle operations
are called without await, which would otherwise silently fail.
"""

import pytest
import warnings
import tempfile
import asyncio
import bundlebase
from conftest import datafile, random_bundle


class TestUnawaitedOperationChain:
    """Test unawaited OperationChain detection."""

    @pytest.mark.asyncio
    async def test_unawaited_operation_chain(self):
        """Test that unawaited OperationChain triggers warning."""
        c = await bundlebase.create(random_bundle())
        c = await c.attach(datafile("userdata.parquet"))

        # Create a chain but don't await it
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            chain = c.filter("id > 100")
            # Let chain go out of scope without awaiting
            del chain

            # Check that warning was issued
            assert len(w) == 1
            assert issubclass(w[0].category, RuntimeWarning)
            assert "filter" in str(w[0].message)
            assert "never awaited" in str(w[0].message)

    @pytest.mark.asyncio
    async def test_unawaited_operation_chain_multiple_ops(self):
        """Test warning for chain with multiple queued operations."""
        c = await bundlebase.create(random_bundle())
        c = await c.attach(datafile("userdata.parquet"))

        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            chain = c.filter("id > 100").remove_column("country")
            del chain

            assert len(w) == 1
            assert "filter" in str(w[0].message)
            assert "remove_column" in str(w[0].message)
            assert "2 operation(s)" in str(w[0].message)

    @pytest.mark.asyncio
    async def test_awaited_operation_chain_no_warning(self):
        """Test that properly awaited chains don't trigger warnings."""
        c = await bundlebase.create(random_bundle())
        c = await c.attach(datafile("userdata.parquet"))

        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            c = await c.filter("id > 100")
            # Should be no warnings
            assert len(w) == 0


class TestUnawaitedCreateChain:
    """Test unawaited CreateChain detection."""

    def test_unawaited_create_chain(self):
        """Test that unawaited CreateChain triggers warning."""
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")

            # Create a chain but don't await it
            chain = bundlebase.create(random_bundle())
            # Let chain go out of scope
            del chain

            # Check warning
            assert len(w) == 1
            assert issubclass(w[0].category, RuntimeWarning)
            assert "create()" in str(w[0].message)
            assert "never awaited" in str(w[0].message)

    def test_unawaited_create_with_operations(self):
        """Test CreateChain with operations that's never awaited."""
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")

            chain = bundlebase.create(random_bundle()).attach(
                datafile("userdata.parquet")
            )
            del chain

            assert len(w) == 1
            assert "attach" in str(w[0].message)
            assert "never awaited" in str(w[0].message)

    def test_unawaited_create_with_multiple_ops(self):
        """Test CreateChain with multiple operations that's never awaited."""
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")

            chain = (
                bundlebase.create(random_bundle())
                .attach(datafile("userdata.parquet"))
                .filter("salary > $1", [50000.0])
                .remove_column("country")
            )
            del chain

            assert len(w) == 1
            assert "attach" in str(w[0].message)
            assert "filter" in str(w[0].message)
            assert "remove_column" in str(w[0].message)
            assert "3 operation(s)" in str(w[0].message)

    @pytest.mark.asyncio
    async def test_awaited_create_chain_no_warning(self):
        """Test that properly awaited create chains don't trigger warnings."""
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")

            c = await bundlebase.create(random_bundle())
            c = await c.attach(datafile("userdata.parquet"))
            c = await c.filter("id > 100")

            # Should be no warnings
            assert len(w) == 0


class TestUnawaitedExtendChain:
    """Test unawaited ExtendChain detection.

    Note: ExtendChain is used for chaining operations after extend(),
    but extend() is used in the synchronous API tests, not the async API.
    """

    @pytest.mark.skip(reason="extend() is for synchronous API, not async")
    async def test_unawaited_extend_chain(self):
        """Test that unawaited ExtendChain with operations triggers warning."""
        pass

    @pytest.mark.skip(reason="extend() is for synchronous API, not async")
    async def test_unawaited_extend_no_operations_no_warning(self):
        """Test that extend() without operations doesn't warn."""
        pass

    @pytest.mark.skip(reason="extend() is for synchronous API, not async")
    async def test_awaited_extend_chain_no_warning(self):
        """Test that properly awaited extend chains don't trigger warnings."""
        pass


class TestWarningContent:
    """Test the content and clarity of warning messages."""

    def test_operation_chain_warning_format(self):
        """Test OperationChain warning message format."""
        async def test():
            c = await bundlebase.create(random_bundle())
            c = await c.attach(datafile("userdata.parquet"))

            with warnings.catch_warnings(record=True) as w:
                warnings.simplefilter("always")
                chain = c.remove_column("country").rename_column("id", "user_id")
                del chain

                assert len(w) == 1
                msg = str(w[0].message)
                assert "OperationChain" in msg
                assert "2 operation(s)" in msg
                assert "remove_column" in msg
                assert "rename_column" in msg
                assert "Did you forget to add 'await'" in msg

        asyncio.run(test())

    def test_create_chain_with_ops_warning_format(self):
        """Test CreateChain with operations warning message format."""
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            chain = bundlebase.create(random_bundle()).attach(
                datafile("userdata.parquet")
            ).filter("salary > $1", [50000.0])
            del chain

            assert len(w) == 1
            msg = str(w[0].message)
            assert "CreateChain" in msg
            assert "create()" in msg
            assert "2 operation(s)" in msg
            assert "attach" in msg
            assert "filter" in msg
            assert "Did you forget to add 'await' before create()" in msg

    def test_create_chain_empty_warning_format(self):
        """Test CreateChain without operations warning message format."""
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            chain = bundlebase.create(random_bundle())
            del chain

            assert len(w) == 1
            msg = str(w[0].message)
            assert "create() was never awaited" in msg
            assert "Did you forget to add 'await' before create()" in msg

    @pytest.mark.skip(reason="extend() is for synchronous API, not async")
    async def test_extend_chain_warning_format(self):
        """Test ExtendChain warning message format."""
        pass


class TestWarningControl:
    """Test that warnings can be controlled via the warnings module."""

    def test_can_filter_warnings(self):
        """Test that warnings can be filtered."""
        # Suppress all RuntimeWarnings
        with warnings.catch_warnings(record=True) as w:
            warnings.filterwarnings("ignore", category=RuntimeWarning)

            chain = bundlebase.create(random_bundle())
            del chain

            # Should have no warnings recorded
            assert len(w) == 0

    def test_warning_message_is_informative(self):
        """Test that warning messages are informative."""
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            chain = bundlebase.create(random_bundle()).attach(
                datafile("userdata.parquet")
            )
            del chain

            assert len(w) == 1
            msg = str(w[0].message)
            # Should have helpful context
            assert "CreateChain" in msg or "create()" in msg
            assert "operation" in msg.lower()
            assert "await" in msg.lower()
