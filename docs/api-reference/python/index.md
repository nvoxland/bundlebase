# Python API Reference

Bundlebase provides both async and sync Python APIs for maximum flexibility.

## API Styles

- **[Async API](async-api.md)** - Modern async/await interface for concurrent operations
- **[Sync API](sync-api.md)** - Synchronous interface for scripts and notebooks
- **[Conversion](conversion.md)** - Export to pandas, polars, numpy, dict
- **[Progress Tracking](progress.md)** - Monitor long-running operations
- **[Operation Chains](chain.md)** - Fluent method chaining

## Quick Navigation

### Creating Bundles

- [`create()`](async-api.md#bundlebase.create) - Create a new bundle
- [`open()`](async-api.md#bundlebase.open) - Open an existing bundle

### Core Classes

- [`PyBundle`](async-api.md#bundlebase.PyBundle) - Read-only bundle
- [`PyBundleBuilder`](async-api.md#bundlebase.PyBundleBuilder) - Mutable bundle
- [`PyBundleStatus`](async-api.md#bundlebase.PyBundleStatus) - Status information
- [`PyChange`](async-api.md#bundlebase.PyChange) - Change tracking

### Utilities

- [`stream_batches()`](conversion.md#bundlebase.conversion.stream_batches) - Stream data efficiently
- [`set_rust_log_level()`](async-api.md#bundlebase.set_rust_log_level) - Configure Rust logging

## Choosing an API Style

### Use the Async API when:

- Building production applications
- Running concurrent operations
- Working with other async libraries
- Need fine-grained control over async execution

```python
import bundlebase

c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("active = true"))

df = await c.to_pandas()
```

### Use the Sync API when:

- Writing simple scripts
- Working in Jupyter notebooks
- Prefer synchronous code style
- Don't need concurrent operations

```python
import bundlebase.sync as dc

c = (dc.create()
    .attach("data.parquet")
    .filter("active = true"))

df = c.to_pandas()
```

## Common Patterns

### Method Chaining

All mutation methods return `self` for fluent chaining:

```python
# Async
c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("age >= 18")
    .remove_column("ssn")
    .rename_column("fname", "first_name"))

# Sync
c = (dc.create()
    .attach("data.parquet")
    .filter("age >= 18")
    .remove_column("ssn")
    .rename_column("fname", "first_name"))
```

### Error Handling

Handle errors with standard Python try/except:

```python
try:
    c = await bundlebase.create()
    c = await c.attach("nonexistent.parquet")
except ValueError as e:
    print(f"Failed to load: {e}")
```

### Streaming Large Datasets

Use streaming for datasets larger than RAM:

```python
import bundlebase

c = await bundlebase.open("huge_dataset.parquet")

async for batch in bundlebase.stream_batches(c):
    # Process batch (~100MB)
    process(batch)
```

## Type Hints

Bundlebase includes comprehensive type hints for IDE support:

```python
from bundlebase import PyBundle, PyBundleBuilder
import pandas as pd

async def process_data(path: str) -> pd.DataFrame:
    """Type-checked function using bundlebase."""
    c: PyBundleBuilder = await bundlebase.create()
    c = await c.attach(path)
    c = await c.filter("active = true")
    df: pd.DataFrame = await c.to_pandas()
    return df
```

## Next Steps

- **[Async API Reference](async-api.md)** - Complete async API documentation
- **[Sync API Reference](sync-api.md)** - Complete sync API documentation
- **[Examples](../../examples/basic-operations.md)** - Practical code examples
- **[Guides](../../guides/architecture.md)** - Deep dives into advanced topics
