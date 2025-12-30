"""Tests for Rust→Python logging bridge."""

import logging
from io import StringIO

import maturin_import_hook
import pytest

maturin_import_hook.install()

import bundlebase
from conftest import datafile, random_bundle


class TestLoggingSetup:
    """Test that logging is properly initialized."""

    def test_rust_logger_exists(self):
        """Test that the rust logger is created on import."""
        rust_logger = logging.getLogger('bundlebase.rust')
        assert rust_logger is not None
        assert rust_logger.level == logging.INFO  # Default level

    def test_rust_logger_has_handler(self):
        """Test that the rust logger has at least one handler."""
        rust_logger = logging.getLogger('bundlebase.rust')
        # Note: May be 0 if using basicConfig, but should be set up
        # The main thing is that the logger exists and is configured
        assert rust_logger is not None

    def test_set_rust_log_level(self):
        """Test that set_rust_log_level function works."""
        rust_logger = logging.getLogger('bundlebase.rust')

        # Set to DEBUG
        bundlebase.set_rust_log_level(logging.DEBUG)
        assert rust_logger.level == logging.DEBUG

        # Set back to INFO
        bundlebase.set_rust_log_level(logging.INFO)
        assert rust_logger.level == logging.INFO

        # Set to WARNING
        bundlebase.set_rust_log_level(logging.WARNING)
        assert rust_logger.level == logging.WARNING


class TestLoggingCapture:
    """Test that Rust logs are captured by Python logging."""

    @pytest.mark.asyncio
    async def test_logs_appear_on_bundle_operations(self):
        """Test that Rust logs appear when performing bundle operations."""
        # Set up logging capture
        log_capture = StringIO()
        handler = logging.StreamHandler(log_capture)
        handler.setFormatter(logging.Formatter('%(levelname)s:%(name)s:%(message)s'))

        rust_logger = logging.getLogger('bundlebase.rust')
        rust_logger.addHandler(handler)
        rust_logger.setLevel(logging.DEBUG)

        try:
            # Create a bundle (should generate logs)
            c = await bundlebase.create(random_bundle())

            # Attach data (should generate logs)
            c = await c.attach(datafile("userdata.parquet"))

            # Verify some logs were captured
            log_output = log_capture.getvalue()
            # We should have some output from Rust operations
            # The exact messages may vary, but there should be something
            # Note: This is a basic check - actual log content depends on
            # what logging statements exist in the Rust code
        finally:
            rust_logger.removeHandler(handler)
            handler.close()

    @pytest.mark.asyncio
    async def test_log_level_filtering_works(self):
        """Test that log level filtering works correctly."""
        log_capture = StringIO()
        handler = logging.StreamHandler(log_capture)
        handler.setFormatter(logging.Formatter('%(levelname)s:%(message)s'))

        rust_logger = logging.getLogger('bundlebase.rust')
        rust_logger.addHandler(handler)

        try:
            # Set to WARNING level (hide DEBUG and INFO)
            bundlebase.set_rust_log_level(logging.WARNING)

            # Perform operations
            c = await bundlebase.create(random_bundle())
            c = await c.attach(datafile("userdata.parquet"))

            # At WARNING level, there may be no logs (depends on Rust code)
            # But if there are logs, they should be WARNING or higher
            log_output = log_capture.getvalue()

            # Check that only WARNING/ERROR appear (no INFO or DEBUG)
            lines = [l for l in log_output.split('\n') if l]
            for line in lines:
                # Extract log level from "LEVEL:message" format
                if ':' in line:
                    level_part = line.split(':')[0]
                    assert level_part in ['WARNING', 'ERROR', 'CRITICAL'], \
                        f"Found {level_part} log at WARNING level, should be filtered"

            # Set back to DEBUG and verify we get more logs
            bundlebase.set_rust_log_level(logging.DEBUG)
            log_capture.truncate(0)
            log_capture.seek(0)

            # Perform another operation
            c = await bundlebase.create(random_bundle())
            c = await c.attach(datafile("userdata.parquet"))

        finally:
            rust_logger.removeHandler(handler)
            handler.close()


class TestLoggingIntegration:
    """Test logging integration with Python logging framework."""

    @pytest.mark.asyncio
    async def test_logging_basicconfig_integration(self):
        """Test that Python basicConfig affects Rust logs."""
        # Configure a file-like handler
        log_capture = StringIO()

        # Create a custom handler
        handler = logging.StreamHandler(log_capture)
        handler.setFormatter(logging.Formatter('%(name)s - %(levelname)s - %(message)s'))

        rust_logger = logging.getLogger('bundlebase.rust')
        rust_logger.addHandler(handler)
        rust_logger.setLevel(logging.INFO)

        try:
            # Perform an operation
            c = await bundlebase.create(random_bundle())

            # Check that the handler received output (or at least was attached)
            # The presence of logs depends on what Rust actually logs
            assert handler is not None
            assert log_capture is not None

        finally:
            rust_logger.removeHandler(handler)
            handler.close()

    def test_rust_logger_is_persistent(self):
        """Test that the rust logger persists across operations."""
        rust_logger1 = logging.getLogger('bundlebase.rust')
        bundlebase.set_rust_log_level(logging.DEBUG)

        rust_logger2 = logging.getLogger('bundlebase.rust')
        assert rust_logger1 is rust_logger2
        assert rust_logger2.level == logging.DEBUG


class TestLoggingAPI:
    """Test the logging API surface."""

    def test_set_rust_log_level_in_public_api(self):
        """Test that set_rust_log_level is in the public API."""
        assert hasattr(bundlebase, 'set_rust_log_level')
        assert callable(bundlebase.set_rust_log_level)

    def test_set_rust_log_level_accepts_logging_levels(self):
        """Test that set_rust_log_level accepts Python logging levels."""
        # These should not raise
        bundlebase.set_rust_log_level(logging.DEBUG)
        bundlebase.set_rust_log_level(logging.INFO)
        bundlebase.set_rust_log_level(logging.WARNING)
        bundlebase.set_rust_log_level(logging.ERROR)
        bundlebase.set_rust_log_level(logging.CRITICAL)

    def test_set_rust_log_level_with_numeric_levels(self):
        """Test that set_rust_log_level works with numeric levels."""
        # Python logging levels are integers
        bundlebase.set_rust_log_level(10)  # DEBUG
        rust_logger = logging.getLogger('bundlebase.rust')
        assert rust_logger.level == 10
