# Custom Functions

Custom functions let you define a Python callable as a data source. Once created, the function can be attached like any other data source using `function://` URLs.

## Creating a Function

Use `create_function()` with:

- `name` -- a unique name for the function
- `output` -- a dict mapping column names to Arrow data type strings
- `func` -- a Python callable that returns data page by page
- `version` -- a version string for tracking changes to the function implementation

The callable receives a `page` number (starting at 0) and a `schema` (PyArrow Schema). Return a `pyarrow.RecordBatch` for each page, or `None` to signal the end of data.

=== "Async API"

    ```python
    import bundlebase as bb
    import pyarrow as pa

    c = await bb.create("my/data")

    def my_data(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
        if page == 0:
            return pa.record_batch(
                {"id": [1, 2, 3], "name": ["Alice", "Bob", "Charlie"]},
                schema=schema,
            )
        return None

    await c.create_function(
        name="my_data",
        output={"id": "Int64", "name": "Utf8"},
        func=my_data,
        version="1",
    )
    await c.attach("function://my_data")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb
    import pyarrow as pa

    c = bb.create("my/data")

    def my_data(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
        if page == 0:
            return pa.record_batch(
                {"id": [1, 2, 3], "name": ["Alice", "Bob", "Charlie"]},
                schema=schema,
            )
        return None

    c.create_function(
        name="my_data",
        output={"id": "Int64", "name": "Utf8"},
        func=my_data,
        version="1",
    )
    c.attach("function://my_data")
    ```

## Output Types

The `output` dict maps column names to [Apache Arrow data type](https://arrow.apache.org/docs/python/api/datatypes.html) strings:

| Type String | Description |
|---|---|
| `Int32`, `Int64` | Integer types |
| `Float32`, `Float64` | Floating point types |
| `Utf8` | String / text |
| `Boolean` | True / false |
| `Date32` | Date |
| `Timestamp` | Date and time |

## Multi-Page Functions

For large datasets, return data across multiple pages:

```python
def paginated_data(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
    if page < 10:  # 10 pages of data
        start = page * 1000
        return pa.record_batch(
            {"id": list(range(start, start + 1000))},
            schema=schema,
        )
    return None  # No more data
```
