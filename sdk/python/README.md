# Bundlebase Python SDK

Build custom Bundlebase connectors in Python.

## Installation

Install the SDK package:

```bash
pip install bundlebase-sdk
```

## Quick Start

Create a custom connector by implementing the `Connector` interface:

```python
from bundlebase_sdk import Connector, Location, serve
import pyarrow as pa

class MyConnector(Connector):
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
    serve(MyConnector())
```

## Implementation

Implement the `Connector` abstract base class:

- **`discover(attached_locations, **kwargs)`** - Return a list of `Location` objects representing available data
- **`data(location, **kwargs)`** - Return data for a location as PyArrow Table, RecordBatch, list of dicts, or iterator
- **`stable_url(location, **kwargs)`** (optional) - Return a stable URL for a location

Call `serve(instance)` to start the connector server.

## Documentation

For complete documentation, including advanced usage and API details, see [Custom Connectors](../../docs/guide/custom-connectors/).
