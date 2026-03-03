# Bundlebase Python SDK

Build custom Bundlebase source functions in Python.

## Installation

Install the SDK package:

```bash
pip install bundlebase-sdk
```

## Quick Start

Create a custom source function by implementing the `SourceFunction` interface:

```python
from bundlebase_sdk import SourceFunction, Location, serve
import pyarrow as pa

class MySource(SourceFunction):
    def discover(self, attached_locations, **kwargs):
        """Return list of available data locations."""
        return [
            Location("data1.parquet", must_copy=True, format="parquet", version="v1"),
            Location("data2.parquet", must_copy=True, format="parquet", version="v1"),
        ]

    def data(self, location, **kwargs):
        """Return data for the given location as PyArrow Table or records."""
        if location.location == "data1.parquet":
            return pa.table({"id": [1, 2, 3], "name": ["alice", "bob", "charlie"]})
        return None

if __name__ == "__main__":
    serve(MySource())
```

## Implementation

Implement the `SourceFunction` abstract base class:

- **`discover(attached_locations, **kwargs)`** - Return a list of `Location` objects representing available data
- **`data(location, **kwargs)`** - Return data for a location as PyArrow Table, RecordBatch, list of dicts, or iterator
- **`stable_url(location, **kwargs)`** (optional) - Return a stable URL for a location

Call `serve(instance)` to start the source function server.

## Documentation

For complete documentation, including advanced usage and API details, see [Custom Source Functions](../../docs/guide/custom-sources/).
