"""Bundlebase - High-performance data processing library with Python bindings."""

import logging
from typing import Any, Callable, Optional, Dict, Awaitable
from ._bundlebase import create as _create
from ._bundlebase import open as _open
from ._bundlebase import test_datafile as test_datafile
from ._bundlebase import random_memory_url as random_memory_url
from ._bundlebase import PyBundle as PyBundle
from ._bundlebase import PyBundleBuilder as _PyBundleBuilder
from ._bundlebase import PyChange as PyChange
from ._bundlebase import PyBundleStatus as PyBundleStatus
from .conversion import to_pandas, to_polars, to_numpy, to_dict, stream_batches
from .chain import OperationChain, register_original_method, CreateChain, ExtendChain
from . import progress

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

    # Column operations
    "remove_column": _PyBundleBuilder.remove_column,
    "rename_column": _PyBundleBuilder.rename_column,
    "select": _PyBundleBuilder.select,

    # Row operations
    "filter": _PyBundleBuilder.filter,
    "join": _PyBundleBuilder.join,
    "attach_to_join": _PyBundleBuilder.attach_to_join,

    # Query operations
    "query": _PyBundleBuilder.query,

    # Metadata operations
    "set_name": _PyBundleBuilder.set_name,
    "set_description": _PyBundleBuilder.set_description,

    # Custom function operations
    "define_function": _PyBundleBuilder.define_function,

    # Index operations
    "define_index": _PyBundleBuilder.index,  # Rust: "index", Python API: "define_index"
    "rebuild_index": _PyBundleBuilder.rebuild_index,
    "reindex": _PyBundleBuilder.reindex,

    # Persistence operations
    "commit": _PyBundleBuilder.commit,

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
mutation_methods = [
    "attach", "remove_column", "rename_column", "filter", "select", "join", "query",
    "set_name", "set_description", "define_function", "define_index",
    "rebuild_index", "reindex"
]
for method_name in mutation_methods:
    wrapped = _wrap_mutation_method(method_name)
    setattr(_PyBundleBuilder, method_name, wrapped)


# Create a reference to the modified class
PyBundleBuilder = _PyBundleBuilder


def create(path: str = "") -> CreateChain:
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
                  .remove_column("unwanted")
                  .rename_column("old", "new"))
    """
    chain = CreateChain(_create, path)
    return chain


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
    return await _open(path)

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

# Wrap extend() on PyBundle to return an ExtendChain
_original_extend = PyBundle.extend
def _wrapped_extend(self, data_dir: str) -> ExtendChain:
    """Extend a bundle to a new directory with chainable operations.

    Args:
        data_dir: Path to the new directory for the extended bundle

    Returns:
        ExtendChain that can be chained with operations

    Example:
        c = await bundlebase.open(path)
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
    "test_datafile",
    "random_memory_url",
    "OperationChain",
    "CreateChain",
    "ExtendChain",
    "progress",
    "set_rust_log_level",
]

__version__ = "0.1.0"