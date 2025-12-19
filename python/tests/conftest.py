"""Shared pytest fixtures and utilities for Bundlebase tests."""

import bundlebase


def datafile(filename: str) -> str:
    """Get memory URL for test data file.

    This function returns a memory:/// URL that points to test data loaded
    into the in-memory object store, matching the behavior of Rust tests.

    Args:
        filename: Name of the file in test_data directory

    Returns:
        Memory URL like "memory:///test_data/userdata.parquet"

    Example:
        >>> c = await c.attach(datafile("userdata.parquet"))
    """
    return bundlebase.test_datafile(filename)


def random_bundle() -> str:
    """Get a unique memory URL for a test bundle.

    Returns a random memory URL for test isolation, ensuring each test
    gets its own bundle that won't conflict with others.

    Returns:
        Random memory URL like "memory:///1234567890"

    Example:
        >>> c = await bundlebase.create(random_bundle())
    """
    return bundlebase.random_memory_url()
