# Conversion Functions

Convert Bundlebase bundles to various data formats.

## Overview

Bundlebase provides utilities to convert bundles to pandas, Polars, NumPy, and Python dictionaries. The conversion functions read data in streaming batches, but the final DataFrame or dict must fit in memory. For datasets larger than RAM, use `stream_batches()`.

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
    .filter("SELECT * FROM bundle WHERE age >= 18"))

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
    .attach("data.parquet"))

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

c = await bundlebase.create().attach("huge_dataset.parquet")

# Process in batches
total_rows = 0
async for batch in bundlebase.stream_batches(c):
    # Each batch is a PyArrow RecordBatch holding a slice of the data
    total_rows += batch.num_rows

    # Convert batch to pandas if needed
    batch_df = batch.to_pandas()
    process(batch_df)

print(f"Processed {total_rows} rows")
```

## Memory Efficiency

The conversion functions read data in streaming batches, but they build the full result in memory. Expect peak memory of roughly twice the dataset size while the batches are combined into the final DataFrame:

```python
# These materialize the full result (dataset must fit in RAM):
df = await c.to_pandas()
df = await c.to_polars()
arrays = await c.to_numpy()

# For datasets larger than RAM, process batch by batch instead:
async for batch in bundlebase.stream_batches(c):
    process(batch)  # bounded memory
```

!!! warning "Avoid for Large Datasets"
    Do NOT use `as_pyarrow()` for large datasets - it materializes the entire dataset in memory. Use `stream_batches()` instead.

## Integration Examples

### With pandas

```python
import bundlebase
import pandas as pd

c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("SELECT * FROM bundle WHERE active = true"))

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
    .attach("data.parquet"))

# Convert to NumPy for numerical operations
arrays = await c.to_numpy()
x = arrays["x"]
y = arrays["y"]
correlation = np.corrcoef(x, y)[0, 1]
```

### Custom Streaming Processing

```python
import bundlebase

c = await bundlebase.create().attach("large_dataset.parquet")

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
