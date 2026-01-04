# Synchronous API Guide

Bundlebase provides a synchronous API (`Bundlebase.sync`) for simple Python scripts and Jupyter notebooks. No `async`/`await` required!

## Overview

The synchronous API wraps the underlying async operations, automatically managing the event loop so you can write clean, pandas-like code:

```python
import Bundlebase.sync as dc

# No async/await needed!
c = dc.create()
c.attach("data.parquet")
c.filter("active = true")
df = c.to_pandas()
print(df.head())
```

## Quick Start

### Installation

Basic installation (scripts only):
```bash
poetry install
```

For Jupyter notebook support:
```bash
poetry install -E jupyter
```

### Simple Script

```python
import Bundlebase.sync as dc

# Create container
c = dc.create()

# Attach data
c.attach("userdata.parquet")

# Transform
c.filter("salary > 50000")
c.remove_column("email")

# Export
df = c.to_pandas()
print(f"Found {len(df)} high earners")
```

### Jupyter Notebook

```python
import Bundlebase.sync as dc

# Works seamlessly in notebooks
c = dc.create()
c.attach("userdata.parquet")

# Display results
display(c.to_pandas())
```

## API Reference

### Factory Functions

#### `create(path: str = "")`

Create a new Bundlebase.

```python
c = dc.create()  # Random memory location
c = dc.create("/path/to/container")  # Specific path
```

**Returns:** `SyncBundlebaseBuilder` (mutable)

#### `open(path: str)`

Open an existing Bundlebase.

```python
c = dc.open("/path/to/saved/container")
```

**Returns:** `SyncBundlebase` (read-only)

#### `stream_batches(container)`

Stream PyArrow RecordBatches from a container.

```python
c = dc.create().attach("large.parquet")
for batch in dc.stream_batches(c):
    print(f"Processing {batch.num_rows} rows")
```

### Container Operations

All operations work synchronously without `await`:

#### Mutation Operations (return `self` for chaining)

- **`attach(url: str)`** - Attach data source
  ```python
  c.attach("data.parquet")
  c.attach("data.csv")
  c.attach("function://my_func")  # Custom function
  ```

- **`remove_column(name: str)`** - Remove column
  ```python
  c.remove_column("unwanted_col")
  ```

- **`rename_column(old: str, new: str)`** - Rename column
  ```python
  c.rename_column("old_name", "new_name")
  ```

- **`filter(expr: str, params: List = [])`** - Filter rows
  ```python
  c.filter("salary > $1", [50000])
  c.filter("status = $1 AND active = true", ["active"])
  ```

- **`select(*columns: str)`** - Select columns
  ```python
  c.select("id", "name", "salary")
  ```

- **`join(name: str, url: str, on: str, how: str = "inner")`** - Join with data
  ```python
  c.join("sales", "sales.parquet", "customer_id = sale_customer", how="left")
  ```

- **`attach_to_join(name: str, url: str)`** - Attach data source for joining
  ```python
  c.attach_to_join("sales", "sales.parquet")
  ```

- **`select(sql: str, params: List = [])`** - Execute SQL
  ```python
  c.select("SELECT * FROM data WHERE id = $1 LIMIT 10", [42])
  ```

- **`set_name(name: str)`** - Set container name
- **`set_description(desc: str)`** - Set description
- **`create_index(column: str)`** - Create index
- **`drop_index(column: str)`** - Drop index
- **`rebuild_index(column: str)`** - Rebuild index

#### Read Operations (no chaining)

- **`num_rows() -> int`** - Get row count
- **`to_pandas() -> DataFrame`** - Convert to pandas
- **`to_polars() -> DataFrame`** - Convert to Polars
- **`to_dict() -> Dict`** - Convert to dict
- **`to_numpy() -> Dict`** - Convert to numpy arrays
- **`explain() -> str`** - Get query plan

#### Properties (no `await`)

- **`schema`** - PyArrow Schema
- **`name`** - Container name
- **`description`** - Container description
- **`version`** - Version hash (12 hex chars)
- **`url`** - Container path

#### Other Methods

- **`history() -> List`** - Commit history
- **`commit(message: str)`** - Save changes
- **`extend(path: str)`** - Create extended copy

## Method Chaining

Since all mutation methods return `self`, you can chain operations without intermediate variables:

```python
df = (dc.create()
      .attach("data.parquet")
      .remove_column("email")
      .filter("active = true")
      .rename_column("fname", "first_name")
      .to_pandas())
```

This is equivalent to:
```python
c = dc.create()
c.attach("data.parquet")
c.remove_column("email")
c.filter("active = true")
c.rename_column("fname", "first_name")
df = c.to_pandas()
```

## Complete Examples

### Data Cleaning Pipeline

```python
import Bundlebase.sync as dc

# Load and clean data
c = dc.create()
c.attach("raw_users.csv")

# Remove personal info for privacy
c.remove_column("email")
c.remove_column("phone")
c.remove_column("address")

# Filter to active users
c.filter("status = $1", ["active"])

# Rename for clarity
c.rename_column("user_id", "id")
c.rename_column("first_name", "fname")
c.rename_column("last_name", "lname")

# Export cleaned data
df = c.to_pandas()
df.to_parquet("cleaned_users.parquet", compression="snappy")
```

### Joining Multiple Data Sources

```python
import Bundlebase.sync as dc

# Start with customers
c = dc.create()
c.attach("customers.csv")

# Add sales data
c.join("sales.csv", "customer_id = sale_customer_id")

# Add geographic data
c.join("regions.csv", "region = region_name")

# Analyze
results = c.to_dict()
print(f"Total customers: {len(results['customer_id'])}")
```

### Streaming Large Datasets

```python
import Bundlebase.sync as dc

# Open large dataset
c = dc.create().attach("100gb_dataset.parquet")

# Process in batches
total_rows = 0
for batch in dc.stream_batches(c):
    # Process batch (doesn't load entire dataset)
    total_rows += batch.num_rows
    print(f"Processed {batch.num_rows} rows")

print(f"Total: {total_rows}")
```

### Saving and Loading Containers

```python
import Bundlebase.sync as dc

# Create and process
c = dc.create("/tmp/my_container")
c.attach("data.parquet")
c.filter("year >= $1", [2020])
c.commit("Filtered to 2020+")

# Later, in another script...
c = dc.open("/tmp/my_container")
df = c.to_pandas()
```

## Comparison: Async vs Sync

### Async API (for advanced use)

```python
import Bundlebase
import asyncio

async def process():
    c = await Bundlebase.create()
    c = await c.attach("data.parquet")
    df = await c.to_pandas()
    return df

asyncio.run(process())
```

### Sync API (simpler!)

```python
import Bundlebase.sync as dc

c = dc.create()
c.attach("data.parquet")
df = c.to_pandas()
```

## Performance

### Overhead

The sync API adds minimal overhead:

- **Scripts:** ~0.1ms per operation (persistent event loop)
- **Jupyter:** ~0.2ms per operation (nested asyncio)

This is negligible compared to data I/O time.

### Chaining Optimization

Chaining multiple operations reduces overhead by executing all operations in a single event loop call:

```python
# Good: One event loop call
df = (dc.create()
      .attach("data.parquet")
      .filter("x > 10")
      .to_pandas())  # Single event loop entry/exit

# Less optimal: Multiple event loop calls
c = dc.create()
c.attach("data.parquet")
c.filter("x > 10")
df = c.to_pandas()  # Multiple event loop entries/exits
```

### Streaming vs Materialization

For large datasets:

```python
# Good: Streams data in batches
for batch in dc.stream_batches(c):
    process_batch(batch)  # Each batch is ~100MB

# Less optimal: Loads all into memory
df = c.to_pandas()  # Entire dataset in RAM
```

## Jupyter Notebook Tips

### Install Jupyter Support

```bash
poetry install -E jupyter
```

Or in a notebook:
```python
%pip install nest-asyncio
```

### Auto-Reload

Enable auto-reload for development:
```python
%load_ext autoreload
%autoreload 2

import Bundlebase.sync as dc
```

### Display DataFrames

```python
import Bundlebase.sync as dc

c = dc.create().attach("data.parquet")
display(c.to_pandas())  # Nice table in notebook
```

## Error Handling

Handle errors like any Python code:

```python
import Bundlebase.sync as dc

try:
    c = dc.create()
    c.attach("nonexistent.parquet")
except ValueError as e:
    print(f"Failed to load data: {e}")

try:
    c.remove_column("nonexistent")
except ValueError as e:
    print(f"Column not found: {e}")
```

## Custom Functions

Define custom data sources:

```python
import Bundlebase.sync as dc
import pyarrow as pa

def my_data_source(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
    """Generate synthetic data."""
    if page == 0:
        return pa.record_batch({
            "id": [1, 2, 3],
            "value": [10, 20, 30]
        }, schema=schema)
    return None

c = dc.create()
c.define_function(
    name="my_func",
    output={"id": "Int32", "value": "Int32"},
    func=my_data_source,
    version="1"
)
c.attach("function://my_func")
df = c.to_pandas()
```

## Migration from Async to Sync

If you have existing async code, migration is straightforward:

### Before (Async)

```python
import Bundlebase
import asyncio

async def process():
    c = await Bundlebase.create()
    c = await c.attach("data.parquet")
    c = await c.filter("x > 10", [10])
    return await c.to_pandas()

df = asyncio.run(process())
```

### After (Sync)

```python
import Bundlebase.sync as dc

c = dc.create()
c.attach("data.parquet")
c.filter("x > 10", [10])
df = c.to_pandas()
```

Just remove `await` and import `Bundlebase.sync` instead of `Bundlebase`!

## Troubleshooting

### ImportError: nest_asyncio required

Install Jupyter support:
```bash
poetry install -E jupyter
```

Or install manually:
```bash
pip install nest-asyncio
```

### "No event loop running" in Jupyter

Make sure you've installed nest-asyncio:
```python
pip install nest-asyncio
```

And imported from Bundlebase.sync:
```python
import Bundlebase.sync as dc  # Not Bundlebase!
```

### Slow performance

Make sure you're chaining operations:

```python
# Good
c = (dc.create()
     .attach("data.parquet")
     .filter("x > 10")
     .to_pandas())

# Slow
c = dc.create()
c.attach("data.parquet")
c.filter("x > 10")
df = c.to_pandas()
```

## See Also

- [02-PYTHON-API.md](02-PYTHON-API.md) - Async API reference
- [05-TESTING.md](05-TESTING.md) - Testing strategy
- [06-DEVELOPMENT.md](06-DEVELOPMENT.md) - Development setup
