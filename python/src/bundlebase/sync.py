"""Synchronous API for Bundle.

Provides a pandas-like synchronous interface for Bundle operations
without requiring explicit async/await syntax. Works seamlessly in both
Python scripts and Jupyter notebooks.

Example:
    >>> import bundlebase.sync as dc
    >>>
    >>> # Create and process data without async/await
    >>> c = dc.create()
    >>> c.attach("data.parquet")
    >>> c.filter("salary > 50000")
    >>> df = c.to_pandas()

For Jupyter notebooks, install with:
    poetry install -E jupyter
"""

from typing import Any, Dict, List, Optional
from bundlebase._loop_manager import EventLoopManager
from bundlebase.chain import _ORIGINAL_METHODS

# Global event loop manager (singleton)
_loop_manager = EventLoopManager()


def _call_original_method(async_bundle: Any, method_name: str, *args: Any, **kwargs: Any) -> Any:
    """Call the original unwrapped method on an async bundle.

    Gets the original method from _ORIGINAL_METHODS (registered before wrapping)
    and returns a coroutine that will call it.

    Args:
        async_bundle: The async bundle to call the method on
        method_name: Name of the method to call
        *args: Positional arguments for the method
        **kwargs: Keyword arguments for the method

    Returns:
        A coroutine that calls the original method
    """
    async def _call_async():
        original_method = _ORIGINAL_METHODS.get(method_name)
        if original_method:
            result = original_method(async_bundle, *args, **kwargs)
        else:
            # Fallback (shouldn't happen if methods are registered)
            result = getattr(async_bundle, method_name)(*args, **kwargs)
        # If it's awaitable (coroutine), await it
        if hasattr(result, '__await__'):
            return await result
        return result

    return _call_async()


class SyncBundle:
    """Synchronous wrapper for PyBundle (read-only).

    Provides a synchronous interface to immutable Bundle operations.
    All async operations are automatically executed synchronously.
    """

    def __init__(self, async_bundle: Any, loop_manager: EventLoopManager) -> None:
        """Initialize synchronous bundle wrapper.

        Args:
            async_bundle: The underlying PyBundle instance
            loop_manager: EventLoopManager for executing async operations
        """
        self._async = async_bundle
        self._loop = loop_manager

    # ======================== Properties ========================
    # These are already synchronous in the async bundle

    @property
    def schema(self) -> Any:
        """Get the schema of the bundle.

        Returns:
            PySchema object representing the current column structure
        """
        coro = _call_original_method(self._async, "schema")
        return self._loop.run_sync(coro)

    @property
    def name(self) -> Optional[str]:
        """Get the bundle name.

        Returns:
            Bundle name or None if not set
        """
        return self._async.name

    @property
    def description(self) -> Optional[str]:
        """Get the bundle description.

        Returns:
            Bundle description or None if not set
        """
        return self._async.description

    @property
    def version(self) -> str:
        """Get the bundle version.

        Returns:
            12-character hex version string
        """
        return self._async.version

    @property
    def url(self) -> str:
        """Get the bundle URL/path.

        Returns:
            Bundle storage location
        """
        return self._async.url

    # ======================== Synchronous Methods ========================

    def history(self) -> List[Any]:
        """Get the commit history of the bundle.

        Returns:
            List of commit objects with metadata
        """
        return self._async.history()

    def status(self) -> List[Any]:
        """Get the list of changes added since bundle creation/extension.

        Returns:
            List of PyChange objects representing uncommitted operations
        """
        return self._async.status()

    # ======================== Async-to-Sync Conversions ========================

    def num_rows(self) -> int:
        """Get the number of rows in the bundle."""
        coro = _call_original_method(self._async, "num_rows")
        return self._loop.run_sync(coro)

    def explain(self) -> str:
        """Get the query execution plan."""
        coro = _call_original_method(self._async, "explain")
        return self._loop.run_sync(coro)

    def to_pandas(self) -> Any:
        """Convert bundle data to pandas DataFrame."""
        coro = _call_original_method(self._async, "to_pandas")
        return self._loop.run_sync(coro)

    def to_polars(self) -> Any:
        """Convert bundle data to Polars DataFrame."""
        coro = _call_original_method(self._async, "to_polars")
        return self._loop.run_sync(coro)

    def to_numpy(self) -> Dict[str, Any]:
        """Convert bundle data to dict of numpy arrays."""
        coro = _call_original_method(self._async, "to_numpy")
        return self._loop.run_sync(coro)

    def to_dict(self) -> Dict[str, List[Any]]:
        """Convert bundle data to dict of lists."""
        coro = _call_original_method(self._async, "to_dict")
        return self._loop.run_sync(coro)

    def as_pyarrow(self) -> Any:
        """Get all data as PyArrow Table."""
        coro = _call_original_method(self._async, "as_pyarrow")
        return self._loop.run_sync(coro)

    def extend(self, data_dir: str) -> "SyncBundleBuilder":
        """Extend this bundle to a new directory.

        Creates a new bundle at data_dir with the same operations and data,
        allowing further modifications without affecting the original.

        Args:
            data_dir: Path where the extended bundle will be stored

        Returns:
            New SyncBundleBuilder

        Raises:
            ValueError: If data_dir is invalid
        """
        coro = _call_original_method(self._async, "extend", data_dir)
        async_extended = self._loop.run_sync(coro)
        return SyncBundleBuilder(async_extended, self._loop)


class SyncBundleBuilder(SyncBundle):
    """Synchronous wrapper for PyBundleBuilder (mutable).

    Provides a synchronous interface to mutable Bundle operations
    with fluent chaining support (no await needed).

    Example:
        >>> c = dc.create()
        >>> c.attach("data.parquet").filter("active = true").remove_column("email")
        >>> df = c.to_pandas()
    """

    # ======================== Mutable Operations ========================
    # All mutation methods return self to enable fluent chaining

    def attach(self, url: str) -> "SyncBundleBuilder":
        """Attach a data source to the bundle."""
        coro = _call_original_method(self._async, "attach", url)
        self._async = self._loop.run_sync(coro)
        return self

    def remove_column(self, name: str) -> "SyncBundleBuilder":
        """Remove a column from the bundle."""
        coro = _call_original_method(self._async, "remove_column", name)
        self._async = self._loop.run_sync(coro)
        return self

    def rename_column(self, old_name: str, new_name: str) -> "SyncBundleBuilder":
        """Rename a column.

        Args:
            old_name: Current column name
            new_name: New column name

        Returns:
            Self for fluent chaining
        """
        coro = _call_original_method(self._async, "rename_column", old_name, new_name)
        self._async = self._loop.run_sync(coro)
        return self

    def filter(self, where_clause: str, params: Optional[List[Any]] = None) -> "SyncBundleBuilder":
        """Filter rows based on a SQL WHERE clause.

        Args:
            where_clause: SQL WHERE condition (e.g., "salary > $1")
            params: Optional list of parameters for parameterized queries.
                    If None, defaults to empty list.

        Returns:
            Self for fluent chaining
        """
        if params is None:
            params = []
        coro = _call_original_method(self._async, "filter", where_clause, params)
        self._async = self._loop.run_sync(coro)
        return self

    def select(self, *columns: str) -> "SyncBundleBuilder":
        """Select specific columns."""
        coro = _call_original_method(self._async, "select", *columns)
        self._async = self._loop.run_sync(coro)
        return self

    def join(
        self, name: str, url: str, on: str, how: str = "inner"
    ) -> "SyncBundleBuilder":
        """Join with another data source."""
        coro = _call_original_method(self._async, "join", name, url, on, how)
        self._async = self._loop.run_sync(coro)
        return self

    def attach_to_join(self, name: str, url: str) -> "SyncBundleBuilder":
        """Attach a data source for joining."""
        coro = _call_original_method(self._async, "attach_to_join", name, url)
        self._async = self._loop.run_sync(coro)
        return self

    def query(self, sql: str, params: Optional[List[Any]] = None) -> "SyncBundleBuilder":
        """Execute a SQL query on the data.

        Args:
            sql: SQL query string
            params: Optional list of parameters for parameterized queries.
                    If None, defaults to empty list.

        Returns:
            Self for fluent chaining
        """
        if params is None:
            params = []
        coro = _call_original_method(self._async, "query", sql, params)
        self._async = self._loop.run_sync(coro)
        return self

    def set_name(self, name: str) -> "SyncBundleBuilder":
        """Set the bundle name."""
        coro = _call_original_method(self._async, "set_name", name)
        self._async = self._loop.run_sync(coro)
        return self

    def set_description(self, desc: str) -> "SyncBundleBuilder":
        """Set the bundle description."""
        coro = _call_original_method(self._async, "set_description", desc)
        self._async = self._loop.run_sync(coro)
        return self

    def define_function(
        self, name: str, output: Dict[str, str], func: Any, version: str = "1"
    ) -> "SyncBundleBuilder":
        """Define a custom Python function as a data source."""
        coro = _call_original_method(self._async, "define_function", name, output, func, version)
        self._async = self._loop.run_sync(coro)
        return self

    def define_index(self, column: str) -> "SyncBundleBuilder":
        """Create an index on a column for faster lookups."""
        coro = _call_original_method(self._async, "define_index", column)
        self._async = self._loop.run_sync(coro)
        return self

    def drop_index(self, column: str) -> "SyncBundleBuilder":
        """Drop an index from a column."""
        coro = _call_original_method(self._async, "drop_index", column)
        self._async = self._loop.run_sync(coro)
        return self

    def rebuild_index(self, column: str) -> "SyncBundleBuilder":
        """Rebuild an existing index on a column."""
        coro = _call_original_method(self._async, "rebuild_index", column)
        self._async = self._loop.run_sync(coro)
        return self

    def reindex(self) -> "SyncBundleBuilder":
        """Create indexes for columns that don't have them yet.

        Iterates through all defined indexes and creates index files for any blocks
        that don't have indexes yet. This is useful after attaching new data or
        recovering from partial index creation failures.

        Returns:
            Self for fluent chaining
        """
        coro = _call_original_method(self._async, "reindex")
        self._async = self._loop.run_sync(coro)
        return self

    def commit(self, message: str) -> Any:
        """Commit changes to persistent storage."""
        coro = _call_original_method(self._async, "commit", message)
        return self._loop.run_sync(coro)


# ======================== Factory Functions ========================


def create(path: str = "") -> SyncBundleBuilder:
    """Create a new Bundle synchronously.

    Creates an empty bundle at the specified path. Use attach() to add data.

    Args:
        path: Optional path for bundle storage (default: random memory location)

    Returns:
        SyncBundleBuilder ready for immediate use

    Example:
        >>> import bundlebase.sync as dc
        >>> c = dc.create()
        >>> c.attach("data.parquet")
        >>> df = c.to_pandas()

    Raises:
        ValueError: If path is invalid
    """
    from bundlebase import _bundlebase

    # Call the underlying async create function directly (returns coroutine)
    async def _create_async():
        return await _bundlebase.create(path)

    async_bundle = _loop_manager.run_sync(_create_async())
    return SyncBundleBuilder(async_bundle, _loop_manager)


def open(path: str) -> SyncBundle:
    """Open an existing Bundle synchronously.

    Loads a previously saved bundle from disk.

    Args:
        path: Path to the saved bundle

    Returns:
        SyncBundle (read-only) with the loaded operations

    Example:
        >>> import bundlebase.sync as dc
        >>> c = dc.open("/path/to/bundle")
        >>> df = c.to_pandas()

    Raises:
        ValueError: If bundle cannot be loaded
    """
    from bundlebase import _bundlebase

    # Call the underlying async open function directly (returns coroutine)
    async def _open_async():
        return await _bundlebase.open(path)

    async_bundle = _loop_manager.run_sync(_open_async())
    return SyncBundle(async_bundle, _loop_manager)


def stream_batches(bundle: SyncBundle) -> Any:
    """Stream RecordBatches from a bundle synchronously.

    WARNING: This function materializes ALL batches in memory first, then yields them.
    This is a limitation of the synchronous API due to Python's threading model.
    For true streaming with constant memory usage, use the async API:
        async for batch in bundlebase.stream_batches(bundle):
            process(batch)

    For better memory efficiency with the sync API, consider:
    1. Using pandas/polars conversion instead of streaming
    2. Processing smaller subsets of data (using filter operations)
    3. Using the async API instead

    Args:
        bundle: SyncBundle to stream from

    Yields:
        pyarrow.RecordBatch objects (all loaded into memory first)

    Example:
        >>> import bundlebase.sync as dc
        >>> c = dc.create().attach("data.parquet")
        >>> for batch in dc.stream_batches(c):
        ...     print(f"Processing {batch.num_rows} rows")

    Raises:
        ValueError: If streaming fails
    """
    import bundlebase

    async def _collect() -> List[Any]:
        """Collect all batches from async stream.

        Note: This collects all batches into memory synchronously,
        which is unavoidable when using sync API with async Rust code.
        """
        batches = []
        async for batch in bundlebase.stream_batches(bundle._async):
            batches.append(batch)
        return batches

    batches = _loop_manager.run_sync(_collect())
    for batch in batches:
        yield batch
