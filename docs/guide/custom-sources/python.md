# Python SDK

Build custom Bundlebase source functions in Python.

## Installation

```bash
pip install bundlebase-sdk
```

Requires `pyarrow` as a peer dependency.

!!! warning "Required Configuration"
    Python source functions require the `allow_external_code` configuration setting. See [Configuration](../custom-sources/index.md#configuration) for details.

## Quick Start

```python
# example_connector.py
import pyarrow as pa
from bundlebase_sdk import SourceFunction, Location, serve

class ExampleConnector(SourceFunction):
    def discover(self, attached_locations, **kwargs):
        return [Location("data.parquet", format="parquet", version="v1")]

    def data(self, location, **kwargs):
        return pa.table({"id": [1, 2, 3], "value": ["a", "b", "c"]})

if __name__ == "__main__":
    serve(ExampleConnector())
```

Use with:

```python
b.create_connector('example.connector')
b.set_temporary_connector_logic('example.connector', type_='ipc', logic='example_connector.py')
b.create_source('example.connector')
```

## API Reference

### SourceFunction

Abstract base class. Subclass and implement `discover()` and `data()`. Optionally override `stable_url()`.

#### `discover(attached_locations, **kwargs) -> list[Location]`

Return the available data locations.

| Parameter | Type | Description |
|-----------|------|-------------|
| `attached_locations` | `list[str]` | Locations already attached to the bundle |
| `**kwargs` | `str` | Extra arguments from the source configuration |

**Returns:** List of `Location` objects.

#### `data(location, **kwargs) -> data`

Return data for the given location.

| Parameter | Type | Description |
|-----------|------|-------------|
| `location` | `Location` | The location to fetch data for |
| `**kwargs` | `str` | Extra arguments from the source configuration |

**Returns:** One of the supported [data return types](#data-return-types).

#### `stable_url(location, **kwargs) -> StableUrl | None`

Return a stable URL for the given location, if available. Default returns `None`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `location` | `Location` | The location to get a URL for |
| `**kwargs` | `str` | Extra arguments from the source configuration |

**Returns:** A `StableUrl` or `None`.

### Location

```python
Location(
    location="path/to/file.parquet",  # identifier (required)
    must_copy=True,                    # copy into bundle? (default: True)
    format="parquet",                  # file format (default: "parquet")
    version="v1",                      # for change detection (default: "")
)
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `location` | `str` | *(required)* | Identifier for this data file |
| `must_copy` | `bool` | `True` | Whether the data must be copied into the bundle |
| `format` | `str` | `"parquet"` | File format |
| `version` | `str` | `""` | Version string for change detection |

### StableUrl

```python
StableUrl(url="https://example.com/data.parquet")
```

| Field | Type | Description |
|-------|------|-------------|
| `url` | `str` | The stable URL string |

### serve()

```python
serve(source: SourceFunction) -> None
```

Run the source function as a JSON-RPC subprocess. Reads requests from stdin and writes responses to stdout. This is the main entry point for source function scripts.

## Data Return Types

The `data()` method supports several return types:

| Return Type | Description |
|------------|-------------|
| `pa.Table` | PyArrow Table (most common) |
| `pa.RecordBatch` | Single record batch |
| `list[pa.RecordBatch]` | Multiple batches (streaming) |
| `list[dict]` | List of row dicts (auto-converted to Arrow) |
| `Iterator[dict]` | Lazy iterator of dicts (auto-converted) |
| `None` | No data for this location |

## Complete Example

A source that discovers multiple locations, returns multi-batch data, provides stable URLs, and handles extra arguments:

```python
import pyarrow as pa
from bundlebase_sdk import SourceFunction, Location, StableUrl, serve


class DatabaseSource(SourceFunction):
    def discover(self, attached_locations, **kwargs):
        db_host = kwargs.get("db_host", "localhost")
        return [
            Location("users.parquet", must_copy=True, format="parquet", version="v2"),
            Location("orders.parquet", must_copy=True, format="parquet", version="v1"),
        ]

    def data(self, location, **kwargs):
        if location.location == "users.parquet":
            # Return multiple batches for large datasets
            batch1 = pa.record_batch(
                {"id": [1, 2], "name": ["alice", "bob"]},
                schema=pa.schema([("id", pa.int64()), ("name", pa.string())]),
            )
            batch2 = pa.record_batch(
                {"id": [3], "name": ["charlie"]},
                schema=pa.schema([("id", pa.int64()), ("name", pa.string())]),
            )
            return [batch1, batch2]

        elif location.location == "orders.parquet":
            return pa.table({
                "order_id": [101, 102],
                "user_id": [1, 2],
                "amount": [29.99, 49.99],
            })

        return None

    def stable_url(self, location, **kwargs):
        if location.location == "users.parquet":
            return StableUrl("https://db.example.com/exports/users-v2.parquet")
        return None


if __name__ == "__main__":
    serve(DatabaseSource())
```

## Testing

The SDK exposes `_serve()` which accepts explicit IO streams, letting you test your source without launching a subprocess:

```python
import io
import json
import struct

import pyarrow as pa

from bundlebase_sdk import SourceFunction, Location
from bundlebase_sdk.serve import _serve


class ExampleConnector(SourceFunction):
    def discover(self, attached_locations, **kwargs):
        return [Location("test.parquet")]

    def data(self, location, **kwargs):
        return pa.table({"x": [1, 2, 3]})


def make_request(method, params=None, req_id=1):
    req = {"jsonrpc": "2.0", "id": req_id, "method": method, "params": params or {}}
    return json.dumps(req).encode() + b"\n"


def test_discover():
    stdin = io.BytesIO(
        make_request("discover", {"attached_locations": []}, req_id=1)
        + make_request("shutdown", req_id=2)
    )
    stdout = io.BytesIO()
    _serve(ExampleConnector(), stdin, stdout)

    resp = json.loads(stdout.getvalue().split(b"\n")[0])
    assert len(resp["result"]["locations"]) == 1
    assert resp["result"]["locations"][0]["location"] == "test.parquet"


def test_data():
    stdin = io.BytesIO(
        make_request("data", {"location": {"location": "test.parquet"}}, req_id=1)
        + make_request("shutdown", req_id=2)
    )
    stdout = io.BytesIO()
    _serve(ExampleConnector(), stdin, stdout)

    out = stdout.getvalue()
    newline_idx = out.index(b"\n") + 1
    length = struct.unpack(">I", out[newline_idx:newline_idx + 4])[0]
    assert length > 0

    ipc_data = out[newline_idx + 4:newline_idx + 4 + length]
    table = pa.ipc.open_stream(ipc_data).read_all()
    assert table.num_rows == 3
```

## Native Mode

Python sources can run in-process for zero-copy Arrow transfer, eliminating subprocess overhead:

```python
import bundlebase.sync as bb
from example_connector import ExampleConnector

bundle = bb.create("my/data")
bundle.create_connector('example.connector')
bundle.set_temporary_connector_logic('example.connector', type_='python', logic='example_connector:ExampleConnector')
bundle.create_source('example.connector')
bundle.fetch("base", "add")
```

The same `SourceFunction` class works for both native and IPC mode — no code changes needed. The only difference is how you register the source logic:

| Mode | Registration | Data Transfer |
|------|-------------|---------------|
| **Native** | `set_temporary_connector_logic(..., type_='python', logic='module:Class')` | Zero-copy via PyO3 |
| **IPC** | `set_temporary_connector_logic(..., type_='ipc', logic='script.py')` | Serialized Arrow IPC over pipes |

### Extra Arguments

Pass extra arguments when creating the source:

```python
bundle.create_source('example.connector', {'db_host': 'prod.example.com'})
```

These are forwarded to your `discover()` and `data()` methods as `**kwargs`, just like in IPC mode.

### When to Use Native vs IPC

**Use native** (`type_='python'`) when:

- Your source is part of the same Python project
- You need maximum performance for large datasets
- You want the simplest possible setup

**Use IPC** (`type_='ipc'`) when:

- Your source runs as a standalone script
- You want process isolation (source crashes don't affect Bundlebase)
- You're packaging your source as a Docker image

See [Native Source Mode](native.md) for the full overview.

## Error Handling

Exceptions raised in your `discover()`, `data()`, or `stable_url()` methods are caught by the SDK and returned as JSON-RPC error responses with code `-32000`. The exception message is included in the error:

```python
class ExampleConnector(SourceFunction):
    def data(self, location, **kwargs):
        raise ValueError("Database connection failed")
        # Bundlebase receives: {"error": {"code": -32000, "message": "Database connection failed"}}
```

Bundlebase surfaces these errors to the user as source function failures during `fetch()`.
