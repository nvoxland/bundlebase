"""Synchronous API for Bundle.

Provides a pandas-like synchronous interface for Bundle operations
without requiring explicit async/await syntax. Works seamlessly in both
Python scripts and Jupyter notebooks.

Example:
    >>> import bundlebase.sync as bb
    >>>
    >>> # Create and process data without async/await
    >>> c = bb.create()
    >>> c.attach("data.parquet")
    >>> c.filter("salary > 50000")
    >>> df = c.to_pandas()

For Jupyter notebooks, install with:
    poetry install -E jupyter
"""

from typing import Any, Dict, List, Optional, TYPE_CHECKING, Iterator

from bundlebase._loop_manager import EventLoopManager
from bundlebase.chain import _ORIGINAL_METHODS

if TYPE_CHECKING:
    from bundlebase import FetchResults
    import pyarrow as pa

# Global event loop manager (singleton)
_loop_manager = EventLoopManager()


class SyncQueryResult:
    """Synchronous wrapper for QueryResult.

    Wraps the async QueryResult to provide synchronous conversion methods.

    Example:
        >>> result = c.query("SELECT * FROM bundle LIMIT 10")
        >>> df = result.to_pandas()
    """

    def __init__(self, async_result: Any) -> None:
        """Initialize with an async QueryResult.

        Args:
            async_result: The underlying async QueryResult
        """
        self._async = async_result

    def to_pandas(self) -> Any:
        """Convert query results to pandas DataFrame."""
        return _loop_manager.run_sync(self._async.to_pandas())

    def to_polars(self) -> Any:
        """Convert query results to Polars DataFrame."""
        return _loop_manager.run_sync(self._async.to_polars())

    def to_dict(self) -> Dict[str, List[Any]]:
        """Convert query results to dict of lists."""
        return _loop_manager.run_sync(self._async.to_dict())

    def stream_batches(self) -> Iterator["pa.RecordBatch"]:
        """Stream batches synchronously.

        WARNING: This materializes ALL batches first, then yields.
        For true streaming, use the async API.
        """
        async def _collect():
            batches = []
            async for batch in self._async.stream_batches():
                batches.append(batch)
            return batches

        batches = _loop_manager.run_sync(_collect())
        for batch in batches:
            yield batch


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

    def __init__(self, async_bundle: Any) -> None:
        """Initialize synchronous bundle wrapper.

        Args:
            async_bundle: The underlying PyBundle instance
        """
        self._async = async_bundle

    # ======================== Properties ========================
    # These are already synchronous in the async bundle

    @property
    def schema(self) -> Any:
        """Get the schema of the bundle.

        Returns:
            PySchema object representing the current column structure
        """
        coro = _call_original_method(self._async, "schema")
        return _loop_manager.run_sync(coro)

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
        return _loop_manager.run_sync(coro)

    def explain(self) -> str:
        """Get the query execution plan."""
        coro = _call_original_method(self._async, "explain")
        return _loop_manager.run_sync(coro)

    def to_pandas(self) -> Any:
        """Convert bundle data to pandas DataFrame."""
        coro = _call_original_method(self._async, "to_pandas")
        return _loop_manager.run_sync(coro)

    def to_polars(self) -> Any:
        """Convert bundle data to Polars DataFrame."""
        coro = _call_original_method(self._async, "to_polars")
        return _loop_manager.run_sync(coro)

    def to_numpy(self) -> Dict[str, Any]:
        """Convert bundle data to dict of numpy arrays."""
        coro = _call_original_method(self._async, "to_numpy")
        return _loop_manager.run_sync(coro)

    def to_dict(self) -> Dict[str, List[Any]]:
        """Convert bundle data to dict of lists."""
        coro = _call_original_method(self._async, "to_dict")
        return _loop_manager.run_sync(coro)

    def as_pyarrow(self) -> Any:
        """Get all data as PyArrow Table."""
        coro = _call_original_method(self._async, "as_pyarrow")
        return _loop_manager.run_sync(coro)

    def extend(
        self,
        data_dir: Optional[str] = None,
    ) -> "SyncBundleBuilder":
        """Extend this bundle to create a new BundleBuilder.

        This is the primary way to create a new BundleBuilder from an existing bundle.
        The new builder can optionally have a different data directory.

        Args:
            data_dir: Optional new data directory. If None, uses the current bundle's data_dir.

        Returns:
            New SyncBundleBuilder

        Example:
            # Extend with just a new data directory
            builder = bundle.extend(data_dir="s3://bucket/new")

            # Extend and then filter
            builder = bundle.extend()
            builder.filter("active = true", [])

        Raises:
            ValueError: If data_dir is invalid
        """
        # extend() is synchronous in Rust, so we call the original method directly
        original_extend = _ORIGINAL_METHODS.get("extend")
        if original_extend:
            async_extended = original_extend(self._async, data_dir)
        else:
            async_extended = self._async.extend(data_dir)
        return SyncBundleBuilder(async_extended)

    def query(self, sql: str, params: Optional[List[Any]] = None) -> SyncQueryResult:
        """Execute a SQL query and return streaming results.

        Unlike extend() with SQL, this does NOT create a new BundleBuilder.
        It directly executes the query and returns the results.

        Args:
            sql: SQL query string
            params: Optional list of parameters for parameterized queries.
                    If None, defaults to empty list.

        Returns:
            SyncQueryResult that can be converted to pandas/polars.
        """
        # Call the wrapped query method which returns QueryResult
        async def _query_async():
            # self._async.query is the wrapped method that returns QueryResult
            return await self._async.query(sql, params)

        async_result = _loop_manager.run_sync(_query_async())
        return SyncQueryResult(async_result)


class SyncBundleBuilder(SyncBundle):
    """Synchronous wrapper for PyBundleBuilder (mutable).

    Provides a synchronous interface to mutable Bundle operations
    with fluent chaining support (no await needed).

    Example:
        >>> c = dc.create()
        >>> c.attach("data.parquet").filter("active = true").drop_column("email")
        >>> df = c.to_pandas()
    """

    # ======================== Mutable Operations ========================
    # All mutation methods return self to enable fluent chaining

    def attach(self, location: str, pack: str = "base") -> "SyncBundleBuilder":
        """Attach a data source to the bundle.

        Args:
            location: The URL/path of the data to attach
            pack: The pack to attach to. Use "base" for the base pack,
                  or a join name to attach to that join's pack.

        Returns:
            Self for fluent chaining
        """
        coro = _call_original_method(self._async, "attach", location, pack)
        self._async = _loop_manager.run_sync(coro)
        return self

    def detach_block(self, location: str) -> "SyncBundleBuilder":
        """Detach a data block from the bundle by its location.

        Args:
            location: The location (URL) of the block to detach

        Returns:
            Self for fluent chaining
        """
        coro = _call_original_method(self._async, "detach_block", location)
        self._async = _loop_manager.run_sync(coro)
        return self

    def replace_block(self, old_location: str, new_location: str) -> "SyncBundleBuilder":
        """Replace a block's data location in the bundle.

        Changes where a block's data is read from without changing the block's
        identity. Useful when data files are moved to a new location.

        Args:
            old_location: The current location (URL) of the block
            new_location: The new location (URL) to read data from

        Returns:
            Self for fluent chaining
        """
        coro = _call_original_method(self._async, "replace_block", old_location, new_location)
        self._async = _loop_manager.run_sync(coro)
        return self

    def drop_column(self, name: str) -> "SyncBundleBuilder":
        """Remove a column from the bundle."""
        coro = _call_original_method(self._async, "drop_column", name)
        self._async = _loop_manager.run_sync(coro)
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
        self._async = _loop_manager.run_sync(coro)
        return self

    def filter(self, query: str, params: Optional[List[Any]] = None) -> "SyncBundleBuilder":
        """Filter rows based on a SQL SELECT query.

        Args:
            query: SQL SELECT query (e.g., "SELECT * FROM bundle WHERE salary > $1")
            params: Optional list of parameters for parameterized queries ($1, $2, etc.).
                    If None, defaults to empty list.

        Returns:
            Self for fluent chaining

        Example:
            >>> c.filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])
            >>> c.filter("SELECT * FROM bundle WHERE active = true")
        """
        if params is None:
            params = []
        coro = _call_original_method(self._async, "filter", query, params)
        self._async = _loop_manager.run_sync(coro)
        return self

    def join(
        self, name: str, on: str, location: Optional[str] = None, how: str = "inner"
    ) -> "SyncBundleBuilder":
        """Join with another data source.

        If location is None, the join point is created without any initial data.
        Data can be attached later using attach(location, pack=name) or create_source(pack=name).
        """
        coro = _call_original_method(self._async, "join", name, on, location, how)
        self._async = _loop_manager.run_sync(coro)
        return self

    def drop_join(self, join_name: str) -> "SyncBundleBuilder":
        """Drop an existing join.

        Args:
            join_name: Name of the join to drop

        Returns:
            Self for fluent chaining
        """
        coro = _call_original_method(self._async, "drop_join", join_name)
        self._async = _loop_manager.run_sync(coro)
        return self

    def create_source(
        self, function: str, args: Dict[str, str], pack: str = "base"
    ) -> "SyncBundleBuilder":
        """Create a data source for a pack.

        Args:
            function: Name of the source function (e.g., "remote_dir", "web_scrape")
            args: Dictionary of arguments for the source function
            pack: Which pack to define the source for:
                - "base" (default): The base pack
                - A join name: A joined pack by its join name

        Returns:
            Self for fluent chaining
        """
        coro = _call_original_method(self._async, "create_source", function, args, pack)
        self._async = _loop_manager.run_sync(coro)
        return self

    def fetch(self, pack: str = "base") -> List["FetchResults"]:
        """Fetch data from sources for a pack.

        Checks the pack's sources for new files and attaches them to the bundle.

        Args:
            pack: Which pack to fetch sources for:
                - "base" (default): The base pack
                - A join name: A joined pack by its join name

        Returns:
            List of FetchResults, one for each source in the pack.
            Each result contains details about blocks added, replaced, and removed.
        """
        coro = _call_original_method(self._async, "fetch", pack)
        return _loop_manager.run_sync(coro)

    def fetch_all(self) -> List["FetchResults"]:
        """Fetch data from all defined sources.

        Checks all defined sources for new files and attaches them to the bundle.

        Returns:
            List of FetchResults, one for each source across all packs.
            Includes results for sources with no changes (empty results).
        """
        coro = _call_original_method(self._async, "fetch_all")
        return _loop_manager.run_sync(coro)

    def extend(
        self,
        data_dir: Optional[str] = None,
    ) -> "SyncBundleBuilder":
        """Extend this bundle to create a new BundleBuilder.

        Args:
            data_dir: Optional new data directory. If None, uses the current bundle's data_dir.

        Returns:
            New SyncBundleBuilder

        Example:
            # Extend and then filter
            extended = c.extend()
            extended.filter("active = true", [])
        """
        # extend() is synchronous in Rust on PyBundleBuilder, call it directly
        async_extended = self._async.extend(data_dir)
        return SyncBundleBuilder(async_extended)

    def query(self, sql: str, params: Optional[List[Any]] = None) -> SyncQueryResult:
        """Execute a SQL query and return streaming results.

        Unlike extend() with SQL, this does NOT create a new BundleBuilder.
        It directly executes the query and returns the results.

        Args:
            sql: SQL query string
            params: Optional list of parameters for parameterized queries.
                    If None, defaults to empty list.

        Returns:
            SyncQueryResult that can be converted to pandas/polars.
        """
        # Call the wrapped query method which returns QueryResult
        async def _query_async():
            return await self._async.query(sql, params)

        async_result = _loop_manager.run_sync(_query_async())
        return SyncQueryResult(async_result)

    def create_view(self, name: str, sql: str) -> "SyncBundleBuilder":
        """Create a view from a SQL query.

        Args:
            name: Name for the new view
            sql: SQL query defining the view contents

        Returns:
            SyncBundleBuilder for the new view
        """
        coro = _call_original_method(self._async, "create_view", name, sql)
        view_async = _loop_manager.run_sync(coro)
        return SyncBundleBuilder(view_async)

    def set_name(self, name: str) -> "SyncBundleBuilder":
        """Set the bundle name."""
        coro = _call_original_method(self._async, "set_name", name)
        self._async = _loop_manager.run_sync(coro)
        return self

    def set_description(self, desc: str) -> "SyncBundleBuilder":
        """Set the bundle description."""
        coro = _call_original_method(self._async, "set_description", desc)
        self._async = _loop_manager.run_sync(coro)
        return self

    def create_function(
        self, name: str, output: Dict[str, str], func: Any, version: str = "1"
    ) -> "SyncBundleBuilder":
        """Define a custom Python function as a data source."""
        coro = _call_original_method(self._async, "create_function", name, output, func, version)
        self._async = _loop_manager.run_sync(coro)
        return self

    def create_index(
        self,
        column: str,
        index_type: str,
        args: Optional[Dict[str, str]] = None
    ) -> "SyncBundleBuilder":
        """Create an index on a column for faster lookups.

        Args:
            column: The column name to index
            index_type: Index type - "column" or "text"
            args: Optional index-specific arguments (e.g., {"tokenizer": "en_stem"} for text indexes)
        """
        coro = _call_original_method(self._async, "create_index", column, index_type, args)
        self._async = _loop_manager.run_sync(coro)
        return self

    def drop_index(self, column: str) -> "SyncBundleBuilder":
        """Drop an index from a column."""
        coro = _call_original_method(self._async, "drop_index", column)
        self._async = _loop_manager.run_sync(coro)
        return self

    def rebuild_index(self, column: str) -> "SyncBundleBuilder":
        """Rebuild an existing index on a column."""
        coro = _call_original_method(self._async, "rebuild_index", column)
        self._async = _loop_manager.run_sync(coro)
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
        self._async = _loop_manager.run_sync(coro)
        return self

    def commit(self, message: str) -> Any:
        """Commit changes to persistent storage."""
        coro = _call_original_method(self._async, "commit", message)
        return _loop_manager.run_sync(coro)


# ======================== Factory Functions ========================


def create(path: str = "", config: Optional[Any] = None) -> SyncBundleBuilder:
    """Create a new Bundle synchronously.

    Creates an empty bundle at the specified path. Use attach() to add data.

    Args:
        path: Optional path for bundle storage (default: random memory location)
        config: Optional configuration (BundleConfig or dict) for cloud storage settings

    Returns:
        SyncBundleBuilder ready for immediate use

    Example:
        >>> import bundlebase.sync as bb
        >>> c = bb.create()
        >>> c.attach("data.parquet")
        >>> df = c.to_pandas()

        >>> # With config:
        >>> config = {"region": "us-west-2"}
        >>> c = dc.create("s3://bucket/", config=config)

    Raises:
        ValueError: If path is invalid
    """
    from bundlebase import _bundlebase

    # Call the underlying async create function directly (returns coroutine)
    async def _create_async():
        return await _bundlebase.create(path, config)

    async_bundle = _loop_manager.run_sync(_create_async())
    return SyncBundleBuilder(async_bundle)


def open(path: str, config: Optional[Any] = None) -> SyncBundle:
    """Open an existing Bundle synchronously.

    Loads a previously saved bundle from disk.

    Args:
        path: Path to the saved bundle
        config: Optional configuration (BundleConfig or dict) for cloud storage settings

    Returns:
        SyncBundle (read-only) with the loaded operations

    Example:
        >>> import bundlebase.sync as bb
        >>> c = bb.open("/path/to/bundle")
        >>> df = c.to_pandas()

        >>> # With config:
        >>> config = {"region": "us-west-2"}
        >>> c = dc.open("s3://bucket/container", config=config)

    Raises:
        ValueError: If bundle cannot be loaded
    """
    from bundlebase import _bundlebase

    # Call the underlying async open function directly (returns coroutine)
    async def _open_async():
        return await _bundlebase.open(path, config)

    async_bundle = _loop_manager.run_sync(_open_async())
    return SyncBundle(async_bundle)


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
        >>> import bundlebase.sync as bb
        >>> c = bb.create().attach("data.parquet")
        >>> for batch in bb.stream_batches(c):
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
