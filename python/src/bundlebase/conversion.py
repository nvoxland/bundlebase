"""Conversion utilities for exporting Bundle data to common Python formats.

This module provides functions to convert Bundle data to various popular
Python data structures (pandas, polars, numpy, dict). All functions are async
and handle the conversion of internal PyArrow RecordBatches to the target format.

**Streaming Support**: All conversion functions now use streaming internally to
handle large datasets efficiently without loading everything into memory.

Example:
    >>> import bundlebase
    >>> c = await bundlebase.create()
    >>> c = await c.attach("data.parquet")
    >>> df = await c.to_pandas()  # Instance method - uses streaming internally
    >>> # Or using the module-level function:
    >>> df = await bundlebase.conversion.to_pandas(c)
    >>>
    >>> # For manual batch processing:
    >>> async for batch in bundlebase.stream_batches(c):
    >>>     # Process each batch independently
    >>>     process(batch)
"""

from typing import TYPE_CHECKING, Any, Optional, Dict, AsyncIterator
import pyarrow as pa

if TYPE_CHECKING:
    import pandas as pd
    import polars as pl
    import numpy as np
    # Avoid circular import for type checking
    from . import PyBundle, PyBundleBuilder


async def stream_batches(bundle: "PyBundle | PyBundleBuilder") -> AsyncIterator[pa.RecordBatch]:
    """Iterate over RecordBatches without materializing the full dataset.

    This function provides streaming access to data, processing one batch at a time.
    This is memory-efficient for large datasets that don't fit in RAM.

    Args:
        bundle: The Bundle to stream from

    Yields:
        pa.RecordBatch: PyArrow RecordBatch objects, one at a time

    Raises:
        ValueError: If streaming fails or bundle is invalid
        TypeError: If bundle doesn't support streaming

    Example:
        >>> c = await bundlebase.open("large_file.parquet")
        >>> total_rows = 0
        >>> async for batch in stream_batches(c):
        >>>     # Process each batch independently
        >>>     df = batch.to_pandas()
        >>>     total_rows += len(df)
        >>>     # Memory is freed after each iteration
        >>> print(f"Processed {total_rows} rows")
    """
    if not hasattr(bundle, 'as_pyarrow_stream'):
        raise TypeError(
            f"Expected PyBundle or PyBundleBuilder with streaming support, "
            f"got {type(bundle).__name__}. Bundle must have as_pyarrow_stream() method."
        )

    try:
        # Get the streaming object from bundle
        stream = await bundle.as_pyarrow_stream()
    except Exception as e:
        raise ValueError(
            f"Failed to create stream from bundle: {e}. "
            "Ensure the bundle is valid and all operations are properly configured."
        ) from e

    # Iterate through batches one at a time
    while True:
        try:
            batch = await stream.next_batch()
            if batch is None:
                break
            yield batch
        except Exception as e:
            raise ValueError(
                f"Error reading batch from stream: {e}. "
                "Data may be corrupted or operations may have failed."
            ) from e


async def _get_arrow_table(bundle: "PyBundle") -> pa.Table:
    """Internal helper to get PyArrow Table with proper error handling.

    This function handles the conversion of record batches to a single Arrow Table,
    with proper error handling for edge cases.

    Args:
        bundle: The Bundle to extract data from

    Returns:
        pa.Table: Combined Arrow Table from all record batches

    Raises:
        ValueError: If query execution fails or bundle is invalid
        RuntimeError: If Arrow conversion fails
        TypeError: If bundle is not a valid PyBundle
    """
    if not hasattr(bundle, 'as_pyarrow'):
        raise TypeError(
            f"Expected PyBundle, got {type(bundle).__name__}. "
            "Bundle must have as_pyarrow() method."
        )

    try:
        # Get record batches from bundle (async)
        record_batches = await bundle.as_pyarrow()
    except Exception as e:
        raise ValueError(
            f"Failed to query bundle data: {e}. "
            "Ensure the bundle is valid and all operations are properly configured."
        ) from e

    if not record_batches:
        raise ValueError(
            "Bundle produced no data. "
            "Check that data sources are configured with attach() and data is available."
        )

    try:
        # Combine record batches into a single table
        arrow_table = pa.Table.from_batches(record_batches)
    except Exception as e:
        raise RuntimeError(
            f"Failed to combine Arrow record batches into table: {e}. "
            "This may indicate incompatible schemas in the data pipeline."
        ) from e

    return arrow_table


async def to_pandas(bundle: "PyBundle | PyBundleBuilder") -> "pd.DataFrame":
    """Convert the bundle's data to a pandas DataFrame using streaming.

    This function uses streaming internally to handle large datasets efficiently.
    Data is processed in batches to avoid loading everything into memory at once.

    Args:
        bundle: The Bundle to convert

    Returns:
        pd.DataFrame: The data as a pandas DataFrame with proper column types

    Raises:
        ImportError: If pandas is not installed
        ValueError: If query execution fails
        RuntimeError: If conversion fails

    Example:
        >>> c = await bundlebase.create()
        >>> c = await c.attach("data.parquet")
        >>> df = await c.to_pandas()  # Using instance method - streams internally
        >>> df.head()
    """
    try:
        import pandas as pd
    except ImportError as e:
        raise ImportError(
            "pandas is required for to_pandas(). "
            "Install it with: pip install pandas"
        ) from e

    # Collect batches using streaming to avoid materializing everything at once
    chunks = []
    async for batch in stream_batches(bundle):
        try:
            chunks.append(batch.to_pandas())
        except Exception as e:
            raise RuntimeError(f"Failed to convert batch to pandas: {e}") from e

    if not chunks:
        raise ValueError(
            "Bundle produced no data. "
            "Check that data sources are configured with attach() and data is available."
        )

    try:
        # Combine all chunks into a single DataFrame
        return pd.concat(chunks, ignore_index=True)
    except Exception as e:
        raise RuntimeError(f"Failed to combine pandas chunks: {e}") from e


async def to_polars(bundle: "PyBundle | PyBundleBuilder") -> "pl.DataFrame":
    """Convert the bundle's data to a Polars DataFrame using streaming.

    This function uses streaming internally to handle large datasets efficiently.
    Polars can efficiently handle batched data construction.

    Args:
        bundle: The Bundle to convert

    Returns:
        pl.DataFrame: The data as a Polars DataFrame

    Raises:
        ImportError: If polars is not installed
        ValueError: If query execution fails
        RuntimeError: If conversion fails

    Example:
        >>> c = await bundlebase.create()
        >>> c = await c.attach("data.parquet")
        >>> df = await c.to_polars()  # Using instance method - streams internally
        >>> df.head()
    """
    try:
        import polars as pl
    except ImportError as e:
        raise ImportError(
            "polars is required for to_polars(). "
            "Install it with: pip install polars"
        ) from e

    # Collect batches using streaming
    batches = []
    async for batch in stream_batches(bundle):
        batches.append(batch)

    if not batches:
        raise ValueError(
            "Bundle produced no data. "
            "Check that data sources are configured with attach() and data is available."
        )

    try:
        # Polars can efficiently construct from multiple batches
        arrow_table = pa.Table.from_batches(batches)
        return pl.from_arrow(arrow_table)
    except Exception as e:
        raise RuntimeError(f"Failed to convert batches to polars: {e}") from e


async def to_numpy(bundle: "PyBundle") -> Dict[str, "np.ndarray"]:
    """Convert the bundle's data to a dictionary of numpy arrays.

    Each column becomes a key in the returned dictionary with a numpy array as its value.
    Null values are preserved (as NaN for float columns, None for object columns).

    Args:
        bundle: The Bundle to convert

    Returns:
        dict: Dictionary mapping column names to numpy arrays

    Raises:
        ImportError: If numpy is not installed (required by PyArrow)
        ValueError: If query execution fails
        RuntimeError: If conversion fails

    Example:
        >>> c = await bundlebase.create()
        >>> c = await c.attach("data.parquet")
        >>> arrays = await c.to_numpy()  # Using instance method
        >>> arrays['column_name']
    """
    try:
        import numpy as np
    except ImportError as e:
        raise ImportError(
            "numpy is required for to_numpy(). "
            "Install it with: pip install numpy"
        ) from e

    arrow_table = await _get_arrow_table(bundle)

    result = {}
    try:
        for i, name in enumerate(arrow_table.schema.names):
            result[name] = arrow_table.column(i).to_numpy()
    except Exception as e:
        raise RuntimeError(f"Failed to convert column '{name}' to numpy: {e}") from e

    return result


async def to_dict(bundle: "PyBundle") -> Dict[str, list]:
    """Convert the bundle's data to a dictionary of lists.

    Each column becomes a key in the returned dictionary with a list of values.
    This is useful for JSON serialization or working with generic Python structures.

    Args:
        bundle: The Bundle to convert

    Returns:
        dict: Dictionary mapping column names to lists of values

    Raises:
        ValueError: If query execution fails
        RuntimeError: If conversion fails

    Example:
        >>> c = await bundlebase.create()
        >>> c = await c.attach("data.parquet")
        >>> data = await c.to_dict()  # Using instance method
        >>> import json
        >>> json.dumps(data)
    """
    arrow_table = await _get_arrow_table(bundle)

    try:
        return arrow_table.to_pydict()
    except Exception as e:
        raise RuntimeError(f"Failed to convert Arrow table to dict: {e}") from e
