"""Bundlebase - High-performance data processing library with Python bindings."""

import logging
from typing import Any, Callable, Optional, Dict, Union

from . import progress
from ._bundlebase import BundleConfig as BundleConfig
from ._bundlebase import PyBundle as PyBundle
from ._bundlebase import PyBundleBuilder as _PyBundleBuilder
from ._bundlebase import PyBundleStatus as PyBundleStatus
from ._bundlebase import PyChange as PyChange
from ._bundlebase import create as _create
from ._bundlebase import log_metrics as log_metrics
from ._bundlebase import open as _open
from ._bundlebase import random_memory_url as random_memory_url
from ._bundlebase import test_datafile as test_datafile
from .chain import OperationChain, register_original_method, CreateChain, ExtendChain
from .conversion import to_pandas, to_polars, to_numpy, to_dict, stream_batches, QueryResult

# Configure Rust→Python logging bridge
_rust_logger = logging.getLogger('bundlebase.rust')
_rust_logger.setLevel(logging.INFO)  # Default level

# Add handler if none exists (for standalone usage)
if not _rust_logger.handlers:
    handler = logging.StreamHandler()
    handler.setFormatter(logging.Formatter(
        '%(levelname)s [%(name)s] %(message)s'
    ))
    _rust_logger.addHandler(handler)


def set_rust_log_level(level: int) -> None:
    """
    Set the logging level for Rust components.

    Args:
        level: logging.DEBUG, logging.INFO, logging.WARNING, logging.ERROR

    Example:
        import logging
        bundlebase.set_rust_log_level(logging.DEBUG)
    """
    _rust_logger.setLevel(level)

# Store and register original methods before wrapping.
#
# IMPORTANT: This dictionary must be kept in sync with the Rust bindings.
# When adding new methods to PyBundleBuilder in the Rust code,
# you MUST add them here to enable:
# 1. Operation chaining via OperationChain
# 2. Sync API access via bundlebase.sync
# 3. Type checking via __init__.pyi
#
# Methods that modify the bundle should return PyBundleBuilder.
# Methods that query (schema, num_rows, explain) should NOT be in this list
# because they have different async patterns and are handled separately.
#
_original_methods = {
    # Data source operations
    "attach": _PyBundleBuilder.attach,
    "detach_block": _PyBundleBuilder.detach_block,
    "replace_block": _PyBundleBuilder.replace_block,
    "create_source": _PyBundleBuilder.create_source,
    "fetch": _PyBundleBuilder.fetch,
    "fetch_all": _PyBundleBuilder.fetch_all,

    # Column operations
    "drop_column": _PyBundleBuilder.drop_column,
    "rename_column": _PyBundleBuilder.rename_column,

    # Row operations
    "filter": _PyBundleBuilder.filter,
    "join": _PyBundleBuilder.join,

    # Query operations
    "query": _PyBundleBuilder.query,

    # View operations
    "create_view": _PyBundleBuilder.create_view,
    "rename_view": _PyBundleBuilder.rename_view,
    "drop_view": _PyBundleBuilder.drop_view,
    "drop_join": _PyBundleBuilder.drop_join,
    "rename_join": _PyBundleBuilder.rename_join,

    # Bundle state operations
    "reset": _PyBundleBuilder.reset,
    "undo": _PyBundleBuilder.undo,

    # Metadata operations
    "set_name": _PyBundleBuilder.set_name,
    "set_description": _PyBundleBuilder.set_description,
    "set_config": _PyBundleBuilder.set_config,

    # Custom function operations
    "create_function": _PyBundleBuilder.create_function,

    # Index operations
    "create_index": _PyBundleBuilder.create_index,
    "drop_index": _PyBundleBuilder.drop_index,
    "rebuild_index": _PyBundleBuilder.rebuild_index,
    "reindex": _PyBundleBuilder.reindex,

    # Persistence operations
    "commit": _PyBundleBuilder.commit,
    "export_tar": _PyBundleBuilder.export_tar,

    # Note: schema, num_rows, explain are NOT in this dict because they:
    # - Return data rather than mutate the bundle
    # - Have async patterns (e.g., async def schema(self))
    # - Exist on both PyBundle and PyBundleBuilder
    # - Are handled separately by the sync wrapper
}

# Register original methods
for method_name, original_method in _original_methods.items():
    register_original_method(method_name, original_method)

# Store original create and open functions
register_original_method("create", _create)
register_original_method("open", _open)


def _wrap_mutation_method(method_name: str) -> Callable[..., OperationChain]:
    """Wrap a mutation method to return an OperationChain.

    Args:
        method_name: Name of the method to wrap

    Returns:
        Wrapped function that returns an OperationChain for fluent chaining
    """

    def wrapper(self: Any, *args: Any, **kwargs: Any) -> OperationChain:
        chain = OperationChain(self)
        # Queue the first operation
        chain._operations.append((method_name, args, kwargs))
        return chain

    wrapper.__name__ = method_name
    original = _original_methods.get(method_name)
    if original:
        wrapper.__doc__ = original.__doc__
    return wrapper


# Wrap mutation methods to return OperationChain
# (but NOT read-only methods like schema, num_rows, explain)
# Note: fetch is NOT here because it returns a value (count), not PyBundleBuilder
# Note: query is NOT here because it returns a stream of results, not PyBundleBuilder
# Note: extend is NOT here because it's synchronous and returns a new builder directly
mutation_methods = [
    "attach", "detach_block", "replace_block", "create_source",
    "drop_column", "rename_column", "filter", "join",
    "create_view", "rename_view", "drop_view", "drop_join", "rename_join",
    "set_name", "set_description", "set_config", "create_function",
    "create_index", "drop_index", "rebuild_index", "reindex",
    "reset", "undo"
]
for method_name in mutation_methods:
    wrapped = _wrap_mutation_method(method_name)
    setattr(_PyBundleBuilder, method_name, wrapped)


# Create a reference to the modified class
PyBundleBuilder = _PyBundleBuilder


ConfigType = Union[BundleConfig, Dict[str, Any]]

def create(path: str = "", config: Optional[ConfigType] = None) -> CreateChain:
    """
    Create a new Bundle with fluent chaining support.

    Returns an awaitable chain that can queue operations before execution.

    Args:
        path: Optional path for bundle storage
        config: Optional configuration (BundleConfig or dict) for cloud storage settings

    Returns:
        CreateChain that can be chained with operations

    Example:
        c = await (create(path)
                  .attach("data.parquet")
                  .drop_column("unwanted")
                  .rename_column("old", "new"))

        # With config:
        config = {"region": "us-west-2", "s3://bucket/": {"endpoint": "http://localhost:9000"}}
        c = await create("s3://my-bucket/", config=config)
    """
    chain = CreateChain(_create, path, config)
    return chain


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

    Example:
        bundle = await open("s3://my-bucket/container")

        # With config:
        config = {"region": "us-west-2"}
        bundle = await open("s3://my-bucket/container", config=config)
    """
    return await _open(path, config)

# Add conversion methods to PyBundle
PyBundle.to_pandas = lambda self: to_pandas(self)
PyBundle.to_polars = lambda self: to_polars(self)
PyBundle.to_numpy = lambda self: to_numpy(self)
PyBundle.to_dict = lambda self: to_dict(self)

# Add conversion methods to PyBundleBuilder
PyBundleBuilder.to_pandas = lambda self: to_pandas(self)
PyBundleBuilder.to_polars = lambda self: to_polars(self)
PyBundleBuilder.to_numpy = lambda self: to_numpy(self)
PyBundleBuilder.to_dict = lambda self: to_dict(self)

# Wrap query() to return QueryResult
_original_bundle_query = PyBundle.query
_original_builder_query = _original_methods["query"]

async def _wrapped_bundle_query(self, sql: str, params=None) -> QueryResult:
    """Execute a SQL query and return streaming results.

    Args:
        sql: SQL query string
        params: Optional list of parameter values for $1, $2 placeholders

    Returns:
        QueryResult with conversion methods (to_pandas, to_polars, to_dict)

    Example:
        result = await bundle.query("SELECT * FROM bundle WHERE id > $1", [100])
        df = await result.to_pandas()
    """
    stream = await _original_bundle_query(self, sql, params)
    return QueryResult(stream)

async def _wrapped_builder_query(self, sql: str, params=None) -> QueryResult:
    """Execute a SQL query and return streaming results.

    Args:
        sql: SQL query string
        params: Optional list of parameter values for $1, $2 placeholders

    Returns:
        QueryResult with conversion methods (to_pandas, to_polars, to_dict)

    Example:
        result = await builder.query("SELECT * FROM bundle WHERE id > $1", [100])
        df = await result.to_pandas()
    """
    stream = await _original_builder_query(self, sql, params)
    return QueryResult(stream)

PyBundle.query = _wrapped_bundle_query
PyBundleBuilder.query = _wrapped_builder_query

# Wrap extend() on PyBundle to return an ExtendChain
_original_extend = PyBundle.extend
register_original_method("extend", _original_extend)

def _wrapped_extend(self, data_dir: Optional[str] = None) -> ExtendChain:
    """Extend a bundle with chainable operations.

    Args:
        data_dir: Optional path to a new directory for the extended bundle.
                  If None, uses the current bundle's data_dir.

    Returns:
        ExtendChain that can be chained with operations

    Example:
        c = await bundlebase.open(path)
        # Extend in same location
        extended = await c.extend().attach("data.parquet")
        # Extend to new location
        extended = await c.extend(new_path).attach("data.parquet")
    """
    return ExtendChain(_original_extend, self, data_dir)

PyBundle.extend = _wrapped_extend

__all__ = [
    "create",
    "open",
    "PyBundle",
    "PyBundleBuilder",
    "PyChange",
    "PyBundleStatus",
    "QueryResult",
    "test_datafile",
    "random_memory_url",
    "OperationChain",
    "CreateChain",
    "ExtendChain",
    "progress",
    "set_rust_log_level",
]

__version__ = "0.1.0"