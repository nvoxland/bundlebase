# Sync API

The sync API provides a synchronous interface perfect for scripts and Jupyter notebooks. No `async`/`await` required!

## Overview

The sync API is available in the `bundlebase.sync` module and wraps the async API with automatic event loop management:

```python
import bundlebase.sync as dc

# No await needed!
c = dc.create()
c.attach("data.parquet")
df = c.to_pandas()
```

## Factory Functions

::: bundlebase.sync.create
    options:
      show_root_heading: true
      show_root_full_path: false

::: bundlebase.sync.open
    options:
      show_root_heading: true
      show_root_full_path: false

## Core Classes

### SyncBundle

Read-only bundle class returned by `bundlebase.sync.open()`.

::: bundlebase.sync.SyncBundle
    options:
      show_root_heading: true
      show_root_full_path: false

### SyncBundleBuilder

Mutable bundle class returned by `bundlebase.sync.create()` and transformation methods.

::: bundlebase.sync.SyncBundleBuilder
    options:
      show_root_heading: true
      show_root_full_path: false

## Utility Functions

::: bundlebase.sync.stream_batches
    options:
      show_root_heading: true
      show_root_full_path: false

## Examples

### Simple Script

```python
import bundlebase.sync as dc

# Create and process data
c = dc.create()
c.attach("userdata.parquet")
c.filter("salary > 50000")
c.remove_column("email")

# Export
df = c.to_pandas()
print(f"Found {len(df)} high earners")
```

### Method Chaining

```python
import bundlebase.sync as dc

df = (dc.create()
      .attach("data.parquet")
      .remove_column("email")
      .filter("active = true")
      .rename_column("fname", "first_name")
      .to_pandas())
```

### Jupyter Notebook

First, install the jupyter extra:

```bash
pip install "bundlebase[jupyter]"
```

Then in your notebook:

```python
import bundlebase.sync as dc

c = dc.create().attach("data.parquet")
display(c.to_pandas())  # Nice table in notebook
```

### Streaming Large Datasets

```python
import bundlebase.sync as dc

c = dc.create().attach("huge_dataset.parquet")

total_rows = 0
for batch in dc.stream_batches(c):
    # Process batch (~100MB)
    total_rows += batch.num_rows
    print(f"Processed {batch.num_rows} rows")

print(f"Total: {total_rows}")
```

### Saving and Loading

```python
import bundlebase.sync as dc

# Create and save
c = dc.create("/tmp/my_bundle")
c.attach("data.parquet")
c.filter("year >= 2020")
c.commit("Filtered to 2020+")

# Later, load
c = dc.open("/tmp/my_bundle")
df = c.to_pandas()
```

## Async vs Sync Comparison

### Async API

```python
import bundlebase
import asyncio

async def process():
    c = await bundlebase.create()
    c = await c.attach("data.parquet")
    df = await c.to_pandas()
    return df

df = asyncio.run(process())
```

### Sync API

```python
import bundlebase.sync as dc

c = dc.create()
c.attach("data.parquet")
df = c.to_pandas()
```

## Performance Notes

### Overhead

The sync API adds minimal overhead:

- **Scripts**: ~0.1ms per operation (persistent event loop)
- **Jupyter**: ~0.2ms per operation (nested asyncio)

This is negligible compared to data I/O time.

### Optimization

Chaining operations reduces overhead:

```python
# Good: One event loop call
df = (dc.create()
      .attach("data.parquet")
      .filter("x > 10")
      .to_pandas())

# Less optimal: Multiple event loop calls
c = dc.create()
c.attach("data.parquet")
c.filter("x > 10")
df = c.to_pandas()
```

## Error Handling

Handle errors like regular Python code:

```python
import bundlebase.sync as dc

try:
    c = dc.create()
    c.attach("nonexistent.parquet")
except ValueError as e:
    print(f"Failed to load data: {e}")
```

## Migration from Async

Migration is straightforward:

### Before (Async)

```python
import bundlebase

async def process():
    c = await bundlebase.create()
    c = await c.attach("data.parquet")
    c = await c.filter("x > 10")
    return await c.to_pandas()
```

### After (Sync)

```python
import bundlebase.sync as dc

c = dc.create()
c.attach("data.parquet")
c.filter("x > 10")
df = c.to_pandas()
```

Just remove `await` and import `bundlebase.sync`!

## Troubleshooting

### ImportError: nest_asyncio required

Install Jupyter support:

```bash
pip install "bundlebase[jupyter]"
```

### "No event loop running" in Jupyter

Make sure you've imported from `bundlebase.sync`:

```python
import bundlebase.sync as dc  # Not bundlebase!
```

## See Also

- **[Async API](async-api.md)** - Async/await interface
- **[Quick Start Guide](../../getting-started/quick-start.md)** - Side-by-side examples
- **[Examples](../../examples/basic-operations.md)** - Practical code examples
