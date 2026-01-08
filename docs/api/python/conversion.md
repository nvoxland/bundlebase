# Conversion Functions

Convert Bundlebase bundles to various data formats.

## Overview

Bundlebase provides utilities to convert bundles to pandas, Polars, NumPy, and Python dictionaries. All conversion functions use streaming internally to handle datasets larger than RAM.

## Functions

### to_pandas

::: bundlebase.conversion.to_pandas
    options:
      show_root_heading: true
      show_root_full_path: false

### to_polars

::: bundlebase.conversion.to_polars
    options:
      show_root_heading: true
      show_root_full_path: false

### to_numpy

::: bundlebase.conversion.to_numpy
    options:
      show_root_heading: true
      show_root_full_path: false

### to_dict

::: bundlebase.conversion.to_dict
    options:
      show_root_heading: true
      show_root_full_path: false

### stream_batches

::: bundlebase.conversion.stream_batches
    options:
      show_root_heading: true
      show_root_full_path: false

## Examples

### Converting to pandas

```python
import bundlebase

c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("age >= 18"))

# Convert to pandas DataFrame
df = await c.to_pandas()
print(df.head())
```

### Converting to Polars

```python
import bundlebase

c = await (bundlebase.create()
    .attach("data.parquet"))

# Convert to Polars DataFrame
df = await c.to_polars()
print(df.describe())
```

### Converting to NumPy

```python
import bundlebase

c = await (bundlebase.create()
    .attach("data.parquet")
    .select(["x", "y", "z"]))

# Convert to NumPy arrays (dict of column name -> array)
arrays = await c.to_numpy()
print(arrays["x"].mean())
```

### Converting to Dictionary

```python
import bundlebase

c = await (bundlebase.create()
    .attach("data.parquet"))

# Convert to Python dict (column name -> list of values)
data = await c.to_dict()
print(len(data["id"]))
```

### Streaming Large Datasets

For datasets larger than RAM, use `stream_batches()`:

```python
import bundlebase

c = await bundlebase.open("huge_dataset.parquet")

# Process in batches
total_rows = 0
async for batch in bundlebase.stream_batches(c):
    # Each batch is a PyArrow RecordBatch (~100MB)
    total_rows += batch.num_rows

    # Convert batch to pandas if needed
    batch_df = batch.to_pandas()
    process(batch_df)

print(f"Processed {total_rows} rows")
```

## Memory Efficiency

All conversion functions use streaming internally:

```python
# These all stream internally (constant memory):
df = await c.to_pandas()   # ✓ Good
df = await c.to_polars()   # ✓ Good
arrays = await c.to_numpy()  # ✓ Good

# For large datasets, use custom streaming:
async for batch in bundlebase.stream_batches(c):
    process(batch)  # ✓ Best for custom processing
```

!!! warning "Avoid for Large Datasets"
    Do NOT use `as_pyarrow()` for large datasets - it materializes the entire dataset in memory. Use `stream_batches()` instead.

## Batch Size

`stream_batches()` uses a default batch size of ~100MB. You can customize this by implementing a custom progress tracker:

```python
from bundlebase.progress import StreamProgress

progress = StreamProgress(batch_size=50_000_000)  # 50MB batches

async for batch in bundlebase.stream_batches(c, progress=progress):
    process(batch)
```

## Integration Examples

### With pandas

```python
import bundlebase
import pandas as pd

c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("active = true"))

# Convert and continue with pandas
df = await c.to_pandas()
df = df.sort_values("date")
df.to_csv("output.csv")
```

### With Polars

```python
import bundlebase
import polars as pl

c = await (bundlebase.create()
    .attach("data.parquet"))

# Convert to Polars for further processing
df = await c.to_polars()
result = df.group_by("category").agg(pl.col("value").sum())
```

### With NumPy

```python
import bundlebase
import numpy as np

c = await (bundlebase.create()
    .attach("data.parquet")
    .select(["x", "y", "z"]))

# Convert to NumPy for numerical operations
arrays = await c.to_numpy()
x = arrays["x"]
y = arrays["y"]
correlation = np.corrcoef(x, y)[0, 1]
```

### Custom Streaming Processing

```python
import bundlebase

c = await bundlebase.open("large_dataset.parquet")

# Custom incremental processing
results = []
async for batch in bundlebase.stream_batches(c):
    # Process each batch independently
    batch_result = analyze_batch(batch)
    results.append(batch_result)

# Combine results
final_result = combine_results(results)
```

## See Also

- **[Async API](async-api.md)** - Bundle operations
- **[Progress Tracking](progress.md)** - Monitor streaming operations
- **[Basic Concepts](../../getting-started/basic-concepts.md#streaming-execution)** - Streaming architecture
