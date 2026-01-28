"""Type stubs for bundlebase module."""

from typing import Any, Callable, Dict, List, Optional, Tuple, Union

__version__: str

class BundleConfig:
    """Configuration for container storage and cloud providers."""

    def __init__(self) -> None: ...

    def set(self, key: str, value: str, url_prefix: Optional[str] = None) -> None:
        """Set a configuration value.

        Args:
            key: Configuration key
            value: Configuration value
            url_prefix: Optional URL prefix for URL-specific config
        """
        ...

ConfigType = Union[BundleConfig, Dict[str, Any]]

def create(path: str = ..., config: Optional[ConfigType] = None) -> "CreateChain":
    """
    Create a new Bundle with fluent chaining support.

    Returns an awaitable chain that can queue operations before execution.

    Args:
        path: Optional path for bundle storage

    Returns:
        CreateChain that can be chained with operations

    Example:
        c = await (create(path)
                  .attach("data.parquet")
                  .drop_column("unwanted"))
    """
    ...

async def open(path: str, config: Optional[ConfigType] = None) -> PyBundle:
    """
    Load a bundle definition from a saved file.

    Args:
        path: Path to the saved bundle file (YAML format)
        config: Optional configuration (BundleConfig or dict) for cloud storage settings

    Returns:
        A PyBundle with the loaded operations (read-only)

    Raises:
        ValueError: If the file cannot be loaded
    """
    ...

class PyBundle:
    """
    Read-only Bundle class for data processing operations.

    Provides a lazy evaluation pipeline for loading, transforming, and querying data
    from various sources using Apache Arrow and DataFusion.

    Note: This class is read-only. Use PyBundleBuilder for mutations.
    """

    async def schema(self) -> "PySchema":
        """
        Get the current schema of the bundle.

        Returns:
            PySchema object representing the current column structure
        """
        ...

    @property
    def num_rows(self) -> int:
        """
        Get the number of rows in the bundle.

        Returns:
            Number of rows based on the attached data sources
        """
        ...

    @property
    def version(self) -> str:
        """
        Get the version of the underlying data.

        Returns:
            Version string
        """
        ...

    def __len__(self) -> int:
        """
        Get the number of rows in the bundle.

        Returns:
            Number of rows (same as num_rows property)
        """
        ...

    @property
    def name(self) -> Optional[str]:
        """
        Get the bundle name.

        Returns:
            Bundle name or None if not set
        """
        ...

    @property
    def description(self) -> Optional[str]:
        """
        Get the bundle description.

        Returns:
            Bundle description or None if not set
        """
        ...

    async def explain(self) -> str:
        """
        Get the query execution plan as a string.

        Generates and returns the logical and physical query plan that DataFusion
        will use to execute the operation pipeline.

        Returns:
            String containing the detailed query execution plan

        Raises:
            ValueError: If plan generation fails

        Example:
            plan = await bundle.explain()
            print(plan)  # Shows the query optimization plan
        """
        ...

    async def to_pandas(self) -> Any:
        """
        Convert the bundle's data to a pandas DataFrame.

        Returns:
            pandas.DataFrame with the results

        Raises:
            ImportError: If pandas is not installed
            ValueError: If conversion fails or bundle has no data

        Example:
            df = await bundle.to_pandas()
        """
        ...

    async def to_polars(self) -> Any:
        """
        Convert the bundle's data to a Polars DataFrame.

        Returns:
            polars.DataFrame with the results

        Raises:
            ImportError: If polars is not installed
            ValueError: If conversion fails or bundle has no data

        Example:
            df = await bundle.to_polars()
        """
        ...

    async def to_numpy(self) -> Dict[str, Any]:
        """
        Convert the bundle's data to a dictionary of numpy arrays.

        Returns:
            Dictionary mapping column names to numpy arrays

        Raises:
            ImportError: If numpy is not installed
            ValueError: If conversion fails or bundle has no data

        Example:
            arrays = await bundle.to_numpy()
        """
        ...

    async def to_dict(self) -> Dict[str, List[Any]]:
        """
        Convert the bundle's data to a dictionary of lists.

        Returns:
            Dictionary mapping column names to lists of values

        Raises:
            ValueError: If conversion fails or bundle has no data

        Example:
            data = await bundle.to_dict()
        """
        ...

    async def as_pyarrow(self) -> Any:
        """
        Execute the operation pipeline and return raw PyArrow RecordBatch results.

        Returns:
            List of PyArrow RecordBatch objects containing the query results

        Raises:
            ValueError: If query execution fails

        Note:
            This is a lower-level method. For most use cases, prefer:
            - to_pandas() for pandas DataFrames
            - to_polars() for Polars DataFrames
            - to_dict() for dictionaries of lists
        """
        ...

    def extend(self, data_dir: str) -> "ExtendChain":
        """
        Extend this bundle to a new directory with chainable operations.

        Creates an BundleBuilder in the specified directory, copying
        the existing bundle's state and allowing new operations to be chained.

        Args:
            data_dir: Path to the new directory for the extended bundle

        Returns:
            ExtendChain that can be chained with operations

        Example:
            c = await bundlebase.open(path)
            extended = await c.extend(new_path).attach("data.parquet").drop_column("col")
        """
        ...


class PyChange:
    """Information about a logical, user-level change."""

    @property
    def id(self) -> str:
        """Unique identifier for the change."""
        ...

    @property
    def description(self) -> str:
        """Human-readable description of what operations were performed."""
        ...

    @property
    def operation_count(self) -> int:
        """Number of individual operations in this change."""
        ...


class PyBundleStatus:
    """Bundle status showing uncommitted changes."""

    @property
    def changes(self) -> List["PyChange"]:
        """The changes that represent changes since creation/extension."""
        ...


    @property
    def total_operations(self) -> int:
        """Total number of individual operations across all changes."""
        ...

    def is_empty(self) -> bool:
        """Check if there are any uncommitted changes."""
        ...


class FetchedBlock:
    """Information about a block that was fetched (added or replaced)."""

    @property
    def attach_location(self) -> str:
        """Location where the block is attached (path in data_dir or URL)."""
        ...

    @property
    def source_location(self) -> str:
        """Original source location identifier."""
        ...


class FetchResults:
    """Results from fetching a single source.

    Contains information about the source and all blocks that were
    added, replaced, or removed during the fetch operation.
    """

    @property
    def source_function(self) -> str:
        """Source function name (e.g., "remote_dir", "web_scrape")."""
        ...

    @property
    def source_url(self) -> str:
        """Source URL or identifier."""
        ...

    @property
    def pack(self) -> str:
        """Pack name ("base" or join name)."""
        ...

    @property
    def added(self) -> List[FetchedBlock]:
        """Blocks that were newly added."""
        ...

    @property
    def replaced(self) -> List[FetchedBlock]:
        """Blocks that were replaced (updated)."""
        ...

    @property
    def removed(self) -> List[str]:
        """Source locations of blocks that were removed."""
        ...

    def total_count(self) -> int:
        """Total number of actions (added + replaced + removed)."""
        ...

    def is_empty(self) -> bool:
        """Check if there were any changes."""
        ...


class PyBundleBuilder:
    """
    Mutable Bundle class for data processing operations.

    Provides a lazy evaluation pipeline for loading, transforming, and querying data
    from various sources using Apache Arrow and DataFusion. Supports fluent operation
    chaining with a single await.

    All mutation methods return an OperationChain that queues operations and can be
    awaited to execute them sequentially.

    Example:
        c = await (await bundlebase.create(path)
                  .attach("data.parquet")
                  .drop_column("unwanted")
                  .rename_column("old_name", "new_name"))
    """

    def status(self) -> "PyBundleStatus":
        """
        Get the bundle status showing uncommitted changes.

        Returns:
            PyBundleStatus object with information about all uncommitted operations

        Example:
            status = bundle.status()
            print(status)  # Shows all changes
            for change in status.changes:
                print(f"Operation: {change.description}")
        """
        ...

    async def schema(self) -> "PySchema":
        """
        Get the current schema of the bundle.

        Returns:
            PySchema object representing the current column structure
        """
        ...

    @property
    def num_rows(self) -> int:
        """
        Get the number of rows in the bundle.

        Returns:
            Number of rows based on the attached data sources
        """
        ...

    @property
    def version(self) -> str:
        """
        Get the version of the underlying data.

        Returns:
            Version string
        """
        ...

    def __len__(self) -> int:
        """
        Get the number of rows in the bundle.

        Returns:
            Number of rows (same as num_rows property)
        """
        ...

    @property
    def name(self) -> Optional[str]:
        """
        Get the bundle name.

        Returns:
            Bundle name or None if not set
        """
        ...

    @property
    def description(self) -> Optional[str]:
        """
        Get the bundle description.

        Returns:
            Bundle description or None if not set
        """
        ...

    def create_function(
        self,
        name: str,
        output: Dict[str, str],
        func: Callable[[int, Any], Any],
        version: str = ...,
    ) -> "OperationChain":
        """
        Define a custom data generation function.

        Queues a function definition operation that will be executed when the chain is awaited.

        Args:
            name: Function name to use in function:// URLs
            output: Dictionary mapping column names to Arrow data types (e.g., {"id": "Int64", "name": "Utf8"})
            func: Python callable that takes (page: int, schema: pyarrow.Schema) and returns RecordBatch or None
            version: Version string for the function implementation

        Returns:
            OperationChain for fluent chaining

        Example:
            def my_data(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
                if page == 0:
                    return pa.record_batch([[1, 2, 3], ["a", "b", "c"]], schema=schema)
                return None

            c = await (c.create_function("my_data", {"id": "Int64", "value": "Utf8"}, my_data))
        """
        ...

    def attach(self, location: str, pack: str = "base") -> "OperationChain":
        """
        Attach data from a file location.

        Queues an attach operation that will be executed when the chain is awaited.
        Supports CSV, JSON, Parquet files, and function:// URLs for custom functions.

        Args:
            location: Data file location (e.g., "data.csv", "data.parquet", "function://my_data")
            pack: Pack to attach to - "base" for the base pack, or a join name for joined data

        Returns:
            OperationChain for fluent chaining

        Raises:
            ValueError: If the location is invalid or data cannot be loaded

        Example:
            c = await c.attach("data.parquet")
            c = await c.attach("extra_users.csv", pack="users")  # attach to a join
        """
        ...

    def detach_block(self, location: str) -> "OperationChain":
        """
        Detach a data block from the bundle by its location.

        Removes a previously attached block from the bundle. The block is
        identified by its location (URL).

        Args:
            location: The location (URL) of the block to detach

        Returns:
            OperationChain for fluent chaining

        Raises:
            ValueError: If no block exists at the specified location

        Example:
            c = await c.detach_block("s3://bucket/data.parquet")
        """
        ...

    def replace_block(self, old_location: str, new_location: str) -> "OperationChain":
        """
        Replace a block's data location in the bundle.

        Changes where a block's data is read from without changing the block's
        identity. Useful when data files are moved to a new location.

        Args:
            old_location: The current location (URL) of the block
            new_location: The new location (URL) to read data from

        Returns:
            OperationChain for fluent chaining

        Raises:
            ValueError: If no block exists at the old location

        Example:
            c = await c.replace_block(
                "s3://old-bucket/data.parquet",
                "s3://new-bucket/data.parquet"
            )
        """
        ...

    def drop_column(self, name: str) -> "OperationChain":
        """
        Queue a drop_column operation.

        Args:
            name: Name of the column to remove

        Returns:
            OperationChain for fluent chaining

        Raises:
            ValueError: If the column doesn't exist

        Example:
            c = await c.drop_column("unwanted_col")
        """
        ...

    def rename_column(self, old_name: str, new_name: str) -> "OperationChain":
        """
        Queue a rename_column operation.

        Args:
            old_name: Current column name
            new_name: New column name

        Returns:
            OperationChain for fluent chaining

        Raises:
            ValueError: If the column doesn't exist

        Example:
            c = await c.rename_column("old_name", "new_name")
        """
        ...

    def set_name(self, name: str) -> "OperationChain":
        """
        Queue a set_name operation.

        Args:
            name: Bundle name

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.set_name("My Bundle")
        """
        ...

    def set_description(self, description: str) -> "OperationChain":
        """
        Queue a set_description operation.

        Args:
            description: Bundle description

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.set_description("A description")
        """
        ...

    def set_config(self, key: str, value: str, url_prefix: Optional[str] = None) -> "OperationChain":
        """
        Queue a set_config operation.

        Args:
            key: Configuration key
            value: Configuration value
            url_prefix: Optional URL prefix for URL-specific config

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.set_config("region", "us-west-2")
            c = await c.set_config("endpoint", "http://localhost:9000", url_prefix="s3://test-bucket/")
        """
        ...

    def filter(self, query: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """
        Queue a filter operation.

        Args:
            query: SQL SELECT query (e.g., "SELECT * FROM bundle WHERE salary > $1")
            params: Optional list of parameters for parameterized queries

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.filter("SELECT * FROM bundle WHERE salary > $1", [50000])
        """
        ...


    def join(self, name: str, expression: str, location: Optional[str] = None, join_type: Optional[str] = None) -> "OperationChain":
        """
        Queue a join operation.

        Args:
            name: Name for the joined data (used to reference in expressions)
            expression: Join condition expression
            location: Optional data file location to join with (can attach data later)
            join_type: Type of join ("Inner", "Left", "Right", "Full")

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.join("users", 'base.id = users.user_id', "users.csv")
            c = await c.join("regions", 'base.country = regions.country')  # attach data later
        """
        ...

    def drop_join(self, join_name: str) -> "OperationChain":
        """
        Drop an existing join.

        Args:
            join_name: Name of the join to drop

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.drop_join("customers")
        """
        ...

    def drop_view(self, view_name: str) -> "OperationChain":
        """
        Drop an existing view.

        Args:
            view_name: Name of the view to drop

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.drop_view("active_users")
        """
        ...

    def drop_index(self, column: str) -> "OperationChain":
        """
        Drop an index from a column.

        Args:
            column: Name of the column whose index to drop

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.drop_index("user_id")
        """
        ...

    def rename_view(self, old_name: str, new_name: str) -> "OperationChain":
        """
        Rename an existing view.

        Args:
            old_name: Current view name
            new_name: New view name

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.rename_view("old_view", "new_view")
        """
        ...

    def rename_join(self, old_name: str, new_name: str) -> "OperationChain":
        """
        Rename an existing join.

        Args:
            old_name: Current join name
            new_name: New join name

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.rename_join("old_join", "new_join")
        """
        ...

    def reset(self) -> "OperationChain":
        """
        Reset all uncommitted changes.

        Reverts the bundle to its last committed state, discarding all
        uncommitted operations.

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.reset()
        """
        ...

    def undo(self) -> "OperationChain":
        """
        Undo the last uncommitted change.

        Removes the most recent uncommitted operation from the bundle.

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.undo()
        """
        ...

    def create_view(self, name: str, sql: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """
        Create a named view from a SQL query.

        Args:
            name: Name for the view
            sql: SQL query defining the view
            params: Optional list of parameters for parameterized queries

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.create_view("active_users", "SELECT * FROM bundle WHERE active = true")
        """
        ...

    def create_index(
        self,
        column: str,
        index_type: str,
        args: Optional[Dict[str, str]] = None
    ) -> "OperationChain":
        """
        Create an index on a column.

        Args:
            column: Name of the column to index
            index_type: Index type - "column" or "text"
            args: Optional index-specific arguments (e.g., {"tokenizer": "en_stem"} for text indexes)

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.create_index("user_id", "column")
            c = await c.create_index("content", "text", {"tokenizer": "en_stem"})
        """
        ...

    def rebuild_index(self, column: str) -> "OperationChain":
        """
        Rebuild an index on a column.

        Args:
            column: Name of the column whose index to rebuild

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.rebuild_index("user_id")
        """
        ...

    def select(self, sql: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """
        Queue a select operation.

        Args:
            sql: SQL query string (e.g., "SELECT * FROM bundle LIMIT 10")
            params: Optional list of parameters for parameterized queries

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.select("SELECT * FROM bundle LIMIT 10")
        """
        ...

    def create_source(self, function: str, args: Dict[str, str], pack: str = "base") -> "OperationChain":
        """
        Create a data source for a pack.

        Queues an operation to define a source from which files can be
        automatically attached via fetch().

        Args:
            function: Source function name. Available functions:

                "remote_dir" - List files from a local or cloud directory:
                    - "url" (required): Directory URL (e.g., "s3://bucket/data/", "file:///path/to/data/")
                    - "patterns" (optional): Comma-separated glob patterns (e.g., "**/*.parquet,**/*.csv")
                    - "copy" (optional): "true" to copy files into bundle (default), "false" to reference in place

                "ftp_directory" - List files from a remote FTP directory:
                    - "url" (required): FTP URL (e.g., "ftp://user:pass@ftp.example.com:21/path/to/data")
                    - "patterns" (optional): Comma-separated glob patterns (defaults to "**/*")
                    Supports anonymous FTP (just omit user/pass), custom ports, and authenticated access.
                    Note: Files are always copied into the bundle (remote files cannot be directly referenced)

                "sftp_directory" - List files from a remote directory via SFTP:
                    - "url" (required): SFTP URL (e.g., "sftp://user@host:22/path/to/data")
                    - "key_path" (required): Path to SSH private key file (e.g., "~/.ssh/id_rsa")
                    - "patterns" (optional): Comma-separated glob patterns (defaults to "**/*")
                    Note: Files are always copied into the bundle (remote files cannot be directly referenced)

            args: Function-specific configuration arguments as described above.
            pack: Which pack to define the source for:
                - "base" (default): The base pack
                - A join name: A joined pack by its join name

        Returns:
            OperationChain for fluent chaining

        Examples:
            # Local/cloud directory (base pack)
            c = await c.create_source("remote_dir", {"url": "s3://bucket/data/", "patterns": "**/*.parquet"})

            # FTP directory (anonymous)
            c = await c.create_source("ftp_directory", {"url": "ftp://ftp.example.com/pub/data/"})

            # FTP directory (authenticated)
            c = await c.create_source("ftp_directory", {"url": "ftp://user:pass@ftp.example.com/data/"})

            # Remote directory via SFTP
            c = await c.create_source("sftp_directory", {"url": "sftp://user@host/data/", "key_path": "~/.ssh/id_rsa"})

            # Define source for a joined pack
            c = await c.create_source("remote_dir", {"url": "s3://bucket/customers/"}, pack="customers")
        """
        ...

    async def fetch(self, pack: str = "base") -> List[FetchResults]:
        """
        Fetch data from sources for a pack.

        Compares files in the pack's sources with already-attached files and
        auto-attaches any new files found.

        Args:
            pack: Which pack to fetch sources for:
                - "base" (default): The base pack
                - A join name: A joined pack by its join name

        Returns:
            List of FetchResults, one for each source in the pack.
            Each result contains details about blocks added, replaced, and removed.

        Example:
            results = await c.fetch()  # Fetch from base pack sources
            for result in results:
                print(f"{result.source_function}: {len(result.added)} added")
        """
        ...

    async def fetch_all(self) -> List[FetchResults]:
        """
        Fetch data from all defined sources.

        Compares files in each source with already-attached files and
        auto-attaches any new files found.

        Returns:
            List of FetchResults, one for each source across all packs.
            Includes results for sources with no changes (empty results).

        Example:
            results = await c.fetch_all()
            for result in results:
                print(f"{result.source_function}: {result.total_count()} changes")
        """
        ...

    async def commit(self, message: str) -> "PyBundleBuilder":
        """
        Commit the current state of the bundle.

        Args:
            message: Commit message describing the changes

        Returns:
            The bundle after committing

        Raises:
            ValueError: If commit fails
        """
        ...

    async def reindex(self) -> "PyBundleBuilder":
        """
        Create indexes for columns that don't have them yet.

        Iterates through all defined indexes and creates index files for any blocks
        that don't have indexes yet. This is useful after attaching new data or
        recovering from partial index creation failures.

        Returns:
            The bundle after reindexing

        Raises:
            ValueError: If reindexing fails
        """
        ...

    async def save(self, path: str) -> None:
        """
        Save the bundle definition to a file.

        Args:
            path: Path to save the bundle definition (YAML format)

        Raises:
            ValueError: If save fails
        """
        ...

    async def explain(self) -> str:
        """
        Get the query execution plan as a string.

        Generates and returns the logical and physical query plan that DataFusion
        will use to execute the operation pipeline.

        Returns:
            String containing the detailed query execution plan

        Raises:
            ValueError: If plan generation fails

        Example:
            plan = await bundle.explain()
            print(plan)  # Shows the query optimization plan
        """
        ...

    async def to_pandas(self) -> Any:
        """
        Convert the bundle's data to a pandas DataFrame.

        Returns:
            pandas.DataFrame with the results

        Raises:
            ImportError: If pandas is not installed
            ValueError: If conversion fails or bundle has no data

        Example:
            df = await bundle.to_pandas()
        """
        ...

    async def to_polars(self) -> Any:
        """
        Convert the bundle's data to a Polars DataFrame.

        Returns:
            polars.DataFrame with the results

        Raises:
            ImportError: If polars is not installed
            ValueError: If conversion fails or bundle has no data

        Example:
            df = await bundle.to_polars()
        """
        ...

    async def to_numpy(self) -> Dict[str, Any]:
        """
        Convert the bundle's data to a dictionary of numpy arrays.

        Returns:
            Dictionary mapping column names to numpy arrays

        Raises:
            ImportError: If numpy is not installed
            ValueError: If conversion fails or bundle has no data

        Example:
            arrays = await bundle.to_numpy()
        """
        ...

    async def to_dict(self) -> Dict[str, List[Any]]:
        """
        Convert the bundle's data to a dictionary of lists.

        Returns:
            Dictionary mapping column names to lists of values

        Raises:
            ValueError: If conversion fails or bundle has no data

        Example:
            data = await bundle.to_dict()
        """
        ...

    async def as_pyarrow(self) -> Any:
        """
        Execute the operation pipeline and return raw PyArrow RecordBatch results.

        Returns:
            List of PyArrow RecordBatch objects containing the query results

        Raises:
            ValueError: If query execution fails

        Note:
            This is a lower-level method. For most use cases, prefer:
            - to_pandas() for pandas DataFrames
            - to_polars() for Polars DataFrames
            - to_dict() for dictionaries of lists
        """
        ...


class OperationChain:
    """
    Awaitable operation chain for fluent Bundle API.

    Allows chaining multiple mutation operations with a single await:

        c = await (c.attach("data.parquet")
                  .drop_column("unwanted")
                  .rename_column("old_name", "new_name"))

    All chained methods return self for continued chaining, and the entire
    chain executes sequentially when awaited.
    """

    def attach(self, location: str, pack: str = "base") -> "OperationChain":
        """Queue an attach operation."""
        ...

    def detach_block(self, location: str) -> "OperationChain":
        """Queue a detach_block operation."""
        ...

    def replace_block(self, old_location: str, new_location: str) -> "OperationChain":
        """Queue a replace_block operation."""
        ...

    def drop_column(self, name: str) -> "OperationChain":
        """Queue a drop_column operation."""
        ...

    def rename_column(self, old_name: str, new_name: str) -> "OperationChain":
        """Queue a rename_column operation."""
        ...

    def filter(self, query: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """Queue a filter operation."""
        ...

    def join(self, name: str, expression: str, location: Optional[str] = None, join_type: Optional[str] = None) -> "OperationChain":
        """Queue a join operation."""
        ...

    def drop_join(self, join_name: str) -> "OperationChain":
        """Queue a drop_join operation."""
        ...

    def drop_view(self, view_name: str) -> "OperationChain":
        """Queue a drop_view operation."""
        ...

    def drop_index(self, column: str) -> "OperationChain":
        """Queue a drop_index operation."""
        ...

    def select(self, sql: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """Queue a select operation."""
        ...

    def set_name(self, name: str) -> "OperationChain":
        """Queue a set_name operation."""
        ...

    def set_description(self, description: str) -> "OperationChain":
        """Queue a set_description operation."""
        ...

    def set_config(self, key: str, value: str, url_prefix: Optional[str] = None) -> "OperationChain":
        """Queue a set_config operation."""
        ...

    def create_function(
        self,
        name: str,
        output: Dict[str, str],
        func: Callable[[int, Any], Any],
        version: str = ...,
    ) -> "OperationChain":
        """Queue a create_function operation."""
        ...

    def create_source(self, function: str, args: Dict[str, str], pack: str = "base") -> "OperationChain":
        """Queue a create_source operation."""
        ...

    def rename_view(self, old_name: str, new_name: str) -> "OperationChain":
        """Queue a rename_view operation."""
        ...

    def rename_join(self, old_name: str, new_name: str) -> "OperationChain":
        """Queue a rename_join operation."""
        ...

    def reset(self) -> "OperationChain":
        """Queue a reset operation."""
        ...

    def undo(self) -> "OperationChain":
        """Queue an undo operation."""
        ...

    def create_view(self, name: str, sql: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """Queue a create_view operation."""
        ...

    def create_index(
        self,
        column: str,
        index_type: str,
        args: Optional[Dict[str, str]] = None
    ) -> "OperationChain":
        """Queue a create_index operation."""
        ...

    def rebuild_index(self, column: str) -> "OperationChain":
        """Queue a rebuild_index operation."""
        ...


class CreateChain:
    """
    Awaitable chain for fluent creation and chaining in one go.

    Handles the special case of creating a bundle first, then chaining operations.
    Unlike OperationChain, this starts without a bundle and creates one first.

    Example:
        c = await (create(path)
                  .attach("data.parquet")
                  .drop_column("unwanted"))
    """

    def attach(self, location: str, pack: str = "base") -> "CreateChain":
        """Queue an attach operation."""
        ...

    def detach_block(self, location: str) -> "CreateChain":
        """Queue a detach_block operation."""
        ...

    def replace_block(self, old_location: str, new_location: str) -> "CreateChain":
        """Queue a replace_block operation."""
        ...

    def drop_column(self, name: str) -> "CreateChain":
        """Queue a drop_column operation."""
        ...

    def rename_column(self, old_name: str, new_name: str) -> "CreateChain":
        """Queue a rename_column operation."""
        ...

    def filter(self, query: str, params: Optional[List[Any]] = None) -> "CreateChain":
        """Queue a filter operation."""
        ...

    def join(self, name: str, expression: str, location: Optional[str] = None, join_type: Optional[str] = None) -> "CreateChain":
        """Queue a join operation."""
        ...

    def drop_join(self, join_name: str) -> "CreateChain":
        """Queue a drop_join operation."""
        ...

    def drop_view(self, view_name: str) -> "CreateChain":
        """Queue a drop_view operation."""
        ...

    def drop_index(self, column: str) -> "CreateChain":
        """Queue a drop_index operation."""
        ...

    def select(self, sql: str, params: Optional[List[Any]] = None) -> "CreateChain":
        """Queue a select operation."""
        ...

    def set_name(self, name: str) -> "CreateChain":
        """Queue a set_name operation."""
        ...

    def set_description(self, description: str) -> "CreateChain":
        """Queue a set_description operation."""
        ...

    def set_config(self, key: str, value: str, url_prefix: Optional[str] = None) -> "CreateChain":
        """Queue a set_config operation."""
        ...

    def create_function(
        self,
        name: str,
        output: Dict[str, str],
        func: Callable[[int, Any], Any],
        version: str = ...,
    ) -> "CreateChain":
        """Queue a create_function operation."""
        ...

    def create_source(self, function: str, args: Dict[str, str], pack: str = "base") -> "CreateChain":
        """Queue a create_source operation."""
        ...

    def rename_view(self, old_name: str, new_name: str) -> "CreateChain":
        """Queue a rename_view operation."""
        ...

    def rename_join(self, old_name: str, new_name: str) -> "CreateChain":
        """Queue a rename_join operation."""
        ...

    def reset(self) -> "CreateChain":
        """Queue a reset operation."""
        ...

    def undo(self) -> "CreateChain":
        """Queue an undo operation."""
        ...

    def create_view(self, name: str, sql: str, params: Optional[List[Any]] = None) -> "CreateChain":
        """Queue a create_view operation."""
        ...

    def create_index(
        self,
        column: str,
        index_type: str,
        args: Optional[Dict[str, str]] = None
    ) -> "CreateChain":
        """Queue a create_index operation."""
        ...

    def rebuild_index(self, column: str) -> "CreateChain":
        """Queue a rebuild_index operation."""
        ...


class ExtendChain:
    """
    Awaitable chain for extending an existing bundle to a new directory.

    Handles the special case of extending an existing bundle, then chaining operations.
    Unlike OperationChain, extend() is synchronous and returns immediately, allowing
    chaining to begin without awaiting first.

    Example:
        c = await bundlebase.open(path)
        extended = await c.extend(new_path).attach("data.parquet").drop_column("unwanted")
    """

    def attach(self, location: str, pack: str = "base") -> "ExtendChain":
        """Queue an attach operation."""
        ...

    def detach_block(self, location: str) -> "ExtendChain":
        """Queue a detach_block operation."""
        ...

    def replace_block(self, old_location: str, new_location: str) -> "ExtendChain":
        """Queue a replace_block operation."""
        ...

    def drop_column(self, name: str) -> "ExtendChain":
        """Queue a drop_column operation."""
        ...

    def rename_column(self, old_name: str, new_name: str) -> "ExtendChain":
        """Queue a rename_column operation."""
        ...

    def filter(self, query: str, params: Optional[List[Any]] = None) -> "ExtendChain":
        """Queue a filter operation."""
        ...

    def join(self, name: str, expression: str, location: Optional[str] = None, join_type: Optional[str] = None) -> "ExtendChain":
        """Queue a join operation."""
        ...

    def drop_join(self, join_name: str) -> "ExtendChain":
        """Queue a drop_join operation."""
        ...

    def drop_view(self, view_name: str) -> "ExtendChain":
        """Queue a drop_view operation."""
        ...

    def drop_index(self, column: str) -> "ExtendChain":
        """Queue a drop_index operation."""
        ...

    def select(self, sql: str, params: Optional[List[Any]] = None) -> "ExtendChain":
        """Queue a select operation."""
        ...

    def set_name(self, name: str) -> "ExtendChain":
        """Queue a set_name operation."""
        ...

    def set_description(self, description: str) -> "ExtendChain":
        """Queue a set_description operation."""
        ...

    def set_config(self, key: str, value: str, url_prefix: Optional[str] = None) -> "ExtendChain":
        """Queue a set_config operation."""
        ...

    def create_function(
        self,
        name: str,
        output: Dict[str, str],
        func: Callable[[int, Any], Any],
        version: str = ...,
    ) -> "ExtendChain":
        """Queue a create_function operation."""
        ...

    def create_source(self, function: str, args: Dict[str, str], pack: str = "base") -> "ExtendChain":
        """Queue a create_source operation."""
        ...

    def rename_view(self, old_name: str, new_name: str) -> "ExtendChain":
        """Queue a rename_view operation."""
        ...

    def rename_join(self, old_name: str, new_name: str) -> "ExtendChain":
        """Queue a rename_join operation."""
        ...

    def reset(self) -> "ExtendChain":
        """Queue a reset operation."""
        ...

    def undo(self) -> "ExtendChain":
        """Queue an undo operation."""
        ...

    def create_view(self, name: str, sql: str, params: Optional[List[Any]] = None) -> "ExtendChain":
        """Queue a create_view operation."""
        ...

    def create_index(
        self,
        column: str,
        index_type: str,
        args: Optional[Dict[str, str]] = None
    ) -> "ExtendChain":
        """Queue a create_index operation."""
        ...

    def rebuild_index(self, column: str) -> "ExtendChain":
        """Queue a rebuild_index operation."""
        ...


class PySchema:
    """Schema information for a bundle."""

    @property
    def fields(self) -> List["PySchemaField"]:
        """
        Get the list of schema fields.

        Returns:
            List of PySchemaField objects
        """
        ...

    def field(self, name: str) -> "PySchemaField":
        """
        Get a specific field by name.

        Args:
            name: Field name to retrieve

        Returns:
            PySchemaField object for the specified field

        Raises:
            ValueError: If field with the given name doesn't exist

        Example:
            field = schema.field("id")
            assert field.data_type == pa.int32()
        """
        ...

    def __len__(self) -> int:
        """Get the number of fields in the schema."""
        ...

    def is_empty(self) -> bool:
        """Check if the schema is empty."""
        ...

    def __str__(self) -> str:
        """Get string representation of the schema."""
        ...


class PySchemaField:
    """Information about a single schema field."""

    @property
    def name(self) -> str:
        """Get the field name."""
        ...

    @property
    def data_type(self) -> Any:
        """
        Get the field's Arrow data type.

        Returns:
            PyArrow DataType object (e.g., pa.int32(), pa.utf8())
        """
        ...

    @property
    def nullable(self) -> bool:
        """Check if the field is nullable."""
        ...

    def __str__(self) -> str:
        """Get string representation of the field."""
        ...

__all__ = [
    "create",
    "open",
    "PyBundle",
    "PyBundleBuilder",
    "PyChange",
    "PyBundleStatus",
    "PySchema",
    "PySchemaField",
    "OperationChain",
    "CreateChain",
]
