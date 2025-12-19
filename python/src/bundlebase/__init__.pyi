"""Type stubs for bundlebase module."""

from typing import Any, Callable, Dict, List, Optional

__version__: str

def create(path: str = ...) -> "CreateChain":
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
                  .remove_column("unwanted"))
    """
    ...

async def open(path: str) -> PyBundle:
    """
    Load a bundle definition from a saved file.

    Args:
        path: Path to the saved bundle file (YAML format)

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
            extended = await c.extend(new_path).attach("data.parquet").remove_column("col")
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
                  .remove_column("unwanted")
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

    def define_function(
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

            c = await (c.define_function("my_data", {"id": "Int64", "value": "Utf8"}, my_data))
        """
        ...

    def attach(self, url: str) -> "OperationChain":
        """
        Attach data from a source URL.

        Queues an attach operation that will be executed when the chain is awaited.
        Supports CSV, JSON, Parquet files, and function:// URLs for custom functions.

        Args:
            url: Data source URL (e.g., "data.csv", "data.parquet", "function://my_data")

        Returns:
            OperationChain for fluent chaining

        Raises:
            ValueError: If the URL is invalid or data cannot be loaded

        Example:
            c = await c.attach("data.parquet")
        """
        ...

    def remove_column(self, name: str) -> "OperationChain":
        """
        Queue a remove_column operation.

        Args:
            name: Name of the column to remove

        Returns:
            OperationChain for fluent chaining

        Raises:
            ValueError: If the column doesn't exist

        Example:
            c = await c.remove_column("unwanted_col")
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

    def filter(self, where_clause: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """
        Queue a filter operation.

        Args:
            where_clause: SQL WHERE clause (e.g., "salary > $1")
            params: Optional list of parameters for parameterized queries

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.filter("salary > $1", [50000])
        """
        ...

    def select(self, *columns: str) -> "OperationChain":
        """
        Queue a select operation.

        Args:
            *columns: Column names or SQL expressions to select

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.select("id", "name", "salary")
        """
        ...

    def join(self, url: str, expression: str, join_type: Optional[str] = None) -> "OperationChain":
        """
        Queue a join operation.

        Args:
            url: Data source URL to join with
            expression: Join condition expression
            join_type: Type of join ("Inner", "Left", "Right", "Full")

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.join("other_data.csv", 'base.id = joined.id')
        """
        ...

    def query(self, sql: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """
        Queue a query operation.

        Args:
            sql: SQL query string (e.g., "SELECT * FROM data LIMIT 10")
            params: Optional list of parameters for parameterized queries

        Returns:
            OperationChain for fluent chaining

        Example:
            c = await c.query("SELECT * FROM data LIMIT 10")
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
                  .remove_column("unwanted")
                  .rename_column("old_name", "new_name"))

    All chained methods return self for continued chaining, and the entire
    chain executes sequentially when awaited.
    """

    def attach(self, url: str) -> "OperationChain":
        """Queue an attach operation."""
        ...

    def remove_column(self, name: str) -> "OperationChain":
        """Queue a remove_column operation."""
        ...

    def rename_column(self, old_name: str, new_name: str) -> "OperationChain":
        """Queue a rename_column operation."""
        ...

    def filter(self, where_clause: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """Queue a filter operation."""
        ...

    def select(self, *columns: str) -> "OperationChain":
        """Queue a select operation."""
        ...

    def join(self, url: str, expression: str, join_type: Optional[str] = None) -> "OperationChain":
        """Queue a join operation."""
        ...

    def query(self, sql: str, params: Optional[List[Any]] = None) -> "OperationChain":
        """Queue a query operation."""
        ...

    def set_name(self, name: str) -> "OperationChain":
        """Queue a set_name operation."""
        ...

    def set_description(self, description: str) -> "OperationChain":
        """Queue a set_description operation."""
        ...

    def define_function(
        self,
        name: str,
        output: Dict[str, str],
        func: Callable[[int, Any], Any],
        version: str = ...,
    ) -> "OperationChain":
        """Queue a define_function operation."""
        ...


class CreateChain:
    """
    Awaitable chain for fluent creation and chaining in one go.

    Handles the special case of creating a bundle first, then chaining operations.
    Unlike OperationChain, this starts without a bundle and creates one first.

    Example:
        c = await (create(path)
                  .attach("data.parquet")
                  .remove_column("unwanted"))
    """

    def attach(self, url: str) -> "CreateChain":
        """Queue an attach operation."""
        ...

    def remove_column(self, name: str) -> "CreateChain":
        """Queue a remove_column operation."""
        ...

    def rename_column(self, old_name: str, new_name: str) -> "CreateChain":
        """Queue a rename_column operation."""
        ...

    def filter(self, where_clause: str, params: Optional[List[Any]] = None) -> "CreateChain":
        """Queue a filter operation."""
        ...

    def select(self, *columns: str) -> "CreateChain":
        """Queue a select operation."""
        ...

    def join(self, url: str, expression: str, join_type: Optional[str] = None) -> "CreateChain":
        """Queue a join operation."""
        ...

    def query(self, sql: str, params: Optional[List[Any]] = None) -> "CreateChain":
        """Queue a query operation."""
        ...

    def set_name(self, name: str) -> "CreateChain":
        """Queue a set_name operation."""
        ...

    def set_description(self, description: str) -> "CreateChain":
        """Queue a set_description operation."""
        ...

    def define_function(
        self,
        name: str,
        output: Dict[str, str],
        func: Callable[[int, Any], Any],
        version: str = ...,
    ) -> "CreateChain":
        """Queue a define_function operation."""
        ...


class ExtendChain:
    """
    Awaitable chain for extending an existing bundle to a new directory.

    Handles the special case of extending an existing bundle, then chaining operations.
    Unlike OperationChain, extend() is synchronous and returns immediately, allowing
    chaining to begin without awaiting first.

    Example:
        c = await bundlebase.open(path)
        extended = await c.extend(new_path).attach("data.parquet").remove_column("unwanted")
    """

    def attach(self, url: str) -> "ExtendChain":
        """Queue an attach operation."""
        ...

    def remove_column(self, name: str) -> "ExtendChain":
        """Queue a remove_column operation."""
        ...

    def rename_column(self, old_name: str, new_name: str) -> "ExtendChain":
        """Queue a rename_column operation."""
        ...

    def filter(self, where_clause: str, params: Optional[List[Any]] = None) -> "ExtendChain":
        """Queue a filter operation."""
        ...

    def select(self, *columns: str) -> "ExtendChain":
        """Queue a select operation."""
        ...

    def join(self, url: str, expression: str, join_type: Optional[str] = None) -> "ExtendChain":
        """Queue a join operation."""
        ...

    def query(self, sql: str, params: Optional[List[Any]] = None) -> "ExtendChain":
        """Queue a query operation."""
        ...

    def set_name(self, name: str) -> "ExtendChain":
        """Queue a set_name operation."""
        ...

    def set_description(self, description: str) -> "ExtendChain":
        """Queue a set_description operation."""
        ...

    def define_function(
        self,
        name: str,
        output: Dict[str, str],
        func: Callable[[int, Any], Any],
        version: str = ...,
    ) -> "ExtendChain":
        """Queue a define_function operation."""
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
