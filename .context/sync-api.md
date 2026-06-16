# Synchronous API Guide

Bundlebase provides a synchronous API (`bundlebase.sync`, implemented in `python/src/bundlebase/sync.py`) for simple Python scripts and Jupyter notebooks. No `async`/`await` required!

## Overview

The synchronous API wraps the underlying async operations, automatically managing the event loop (via `EventLoopManager` in `_loop_manager.py`) so you can write clean, pandas-like code:

```python
import bundlebase.sync as bb

# No async/await needed!
c = bb.create()
c.attach("data.parquet")
c.filter("SELECT * FROM bundle WHERE active = true")
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
import bundlebase.sync as bb

# Create bundle
c = bb.create()

# Attach data
c.attach("userdata.parquet")

# Transform
c.filter("SELECT * FROM bundle WHERE salary > 50000")
c.drop_column("email")

# Export
df = c.to_pandas()
print(f"Found {len(df)} high earners")
```

### Jupyter Notebook

```python
import bundlebase.sync as bb

# Works seamlessly in notebooks
c = bb.create()
c.attach("userdata.parquet")

# Display results
display(c.to_pandas())
```

## API Reference

### Factory Functions

#### `create(path: str = "", config: Optional[dict] = None)`

Create a new bundle.

```python
c = bb.create()  # Random memory location
c = bb.create("/path/to/bundle")  # Specific path
```

**Returns:** `SyncBundleBuilder` (mutable)

#### `open(path: str, config: Optional[dict] = None)`

Open an existing bundle.

```python
c = bb.open("/path/to/saved/bundle")
```

**Returns:** `SyncBundle` (read-only). Use `extend()` to get a mutable `SyncBundleBuilder`.

#### `upgrade_bundle(path: str, config: Optional[dict] = None)`

Upgrade a bundle's format version when opening fails due to a version mismatch.

#### `stream_batches(bundle)`

Module-level helper to stream PyArrow RecordBatches from a bundle. (There is also a `stream_batches()` method on `SyncBundle`, see below.)

```python
c = bb.create().attach("large.parquet")
for batch in bb.stream_batches(c):
    print(f"Processing {batch.num_rows} rows")
```

### Bundle Operations

All operations work synchronously without `await`:

#### Mutation Operations (return `self` for chaining)

- **`attach(location: str, pack: str = "base")`** - Attach data source
  ```python
  c.attach("data.parquet")
  c.attach("data.csv")
  c.attach("orders.parquet", pack="orders")  # attach to a join's pack
  ```

- **`detach_block(location)` / `replace_block(old_location, new_location)`** - Remove or repoint an attached block

- **`drop_column(name: str)`** - Remove column
  ```python
  c.drop_column("unwanted_col")
  ```

- **`rename_column(old_name: str, new_name: str)`** - Rename column
  ```python
  c.rename_column("old_name", "new_name")
  ```

- **`normalize_column_names()`** - Convert all column names to lowercase+underscore identifiers

- **`add_column(name: str, expression: str)`** - Add a computed column
  ```python
  c.add_column("full_name", "first_name || ' ' || last_name")
  ```

- **`cast_column(name: str, new_type: str)`** - Cast column type
  ```python
  c.cast_column("price", "integer")
  ```

- **`filter(query: str, params: Optional[List] = None)`** - Filter rows with a full SQL SELECT query
  ```python
  c.filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])
  c.filter("SELECT * FROM bundle WHERE active = true")
  ```

- **`delete(where_clause: str)`** / **`update(set_where: str)`** - Delete/update rows
  ```python
  c.delete("salary < 0")
  c.update("SET salary = 100 WHERE id = 1")
  ```

- **`always_delete(where_clause)` / `always_update(set_where)`** - Register persistent rules applied to future attaches (`drop_always_delete()` / `drop_always_update()` to remove)

- **`join(name: str, expression: str, location: Optional[str] = None, join_type: str = "inner")`** - Join with another data source. With `location=None`, creates the join point only; attach data later with `attach(loc, pack=name)`.
  ```python
  c.join("sales", "customer_id = sale_customer", "sales.parquet", join_type="left")
  ```
  (`drop_join(name)` removes a join.)

- **`create_source(connector, args, pack="base", save_as=None, min_batch=None, *, fetch=True)`** - Define a connector-backed data source; **`fetch(pack="base", mode="add")`** / **`fetch_all(mode="add")`** pull new data from sources (return `FetchResults`, not self)

- **`create_view(name: str, sql: str)`** - Create a named view (returns the view's builder)
  ```python
  c.create_view("adults", "SELECT * FROM bundle WHERE age > 21")
  ```

- **`set_name(name: str)`** - Set bundle name
- **`set_description(desc: str)`** - Set description
- **`create_index(columns, index_type, args=None, name=None)`** - Create index (`index_type`: "btree" or "text")
- **`drop_index(column)` / `rebuild_index(column)` / `reindex()`** - Index maintenance
- **`create_report(id, name, description, content)`** - Create or replace a stored report template
- **`drop_report(id)`** - Drop a stored report
- **`generate_report(id, output, no_branding=False) -> str`** - Generate PDF from stored report
- **`import_function` / `import_temp_function` / `import_connector` / `import_temp_connector`** - Register custom functions/connectors (see Custom Functions below)
- **`export_tar(tar_path, *, gzip=False)`** - Export bundle as single tar archive
- **`export_empty(path)`** - Export bundle structure (no data) for recipients to fetch themselves

#### Query and Read Operations (no chaining)

- **`query(sql: str, params: Optional[List] = None, hard_limit: Optional[int] = None) -> SyncQueryResult`** - Execute SQL
  ```python
  result = c.query("SELECT id, name FROM bundle WHERE id = $1 LIMIT 10", [42])
  df = result.to_pandas()
  ```
  `SyncQueryResult` provides `to_pandas()`, `to_polars()`, `to_pyarrow()`, `to_dict()`, and `stream_batches()`.

- **`num_rows() -> int`** - Get row count
- **`to_pandas() -> DataFrame`** - Convert to pandas (materializes the full DataFrame)
- **`to_polars() -> DataFrame`** - Convert to Polars (materializes)
- **`to_dict() -> Dict`** - Convert to dict
- **`to_numpy() -> Dict`** - Convert to numpy arrays
- **`as_pyarrow()`** - Get all data as PyArrow batches (materializes; prefer `stream_batches()`)
- **`stream_batches(sql: Optional[str] = None)`** - Yield RecordBatches one at a time (bounded memory). With `sql`, streams that query's results instead of the full bundle.
- **`explain(verbose=False, analyze=False, format=None, sql=None) -> ExplainResult`** - Get query plan (printable result)
- **`describe_data(columns) -> SyncQueryResult`** - Per-column quality stats
- **`describe_function(name) -> SyncQueryResult`** - Registered function metadata
- **`test_connector(name, **kwargs)` / `test_temp_connector(from_, **kwargs)`** - Validate a connector without modifying the bundle

#### Properties (no parentheses)

- **`schema`** - PySchema object (fields, types, nullability)
- **`name`** - Bundle name
- **`description`** - Bundle description
- **`version`** - Version hash (12 hex chars)
- **`url`** - Bundle path

#### Other Methods

- **`history() -> List`** - Commit history
- **`status()`** - Uncommitted changes
- **`commit(message: str)`** - Save changes
- **`extend(data_dir: Optional[str] = None) -> SyncBundleBuilder`** - Create a mutable builder from a bundle (optionally at a new location)
- **`view(identifier: str) -> SyncBundle`** - Open a named view as a read-only sub-bundle
- **`views() -> Dict[str, str]`** - `{view_id: view_name}` for every view

## Method Chaining

Since all mutation methods return `self`, you can chain operations without intermediate variables:

```python
df = (bb.create()
      .attach("data.parquet")
      .drop_column("email")
      .filter("SELECT * FROM bundle WHERE active = true")
      .rename_column("fname", "first_name")
      .to_pandas())
```

This is equivalent to:
```python
c = bb.create()
c.attach("data.parquet")
c.drop_column("email")
c.filter("SELECT * FROM bundle WHERE active = true")
c.rename_column("fname", "first_name")
df = c.to_pandas()
```

Note: unlike the async `OperationChain`, sync chaining is purely cosmetic — each method call runs its operation immediately on the shared event loop before returning `self`.

## Complete Examples

### Data Cleaning Pipeline

```python
import bundlebase.sync as bb

# Load and clean data
c = bb.create()
c.attach("raw_users.csv")

# Remove personal info for privacy
c.drop_column("email")
c.drop_column("phone")
c.drop_column("address")

# Filter to active users
c.filter("SELECT * FROM bundle WHERE status = $1", ["active"])

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
import bundlebase.sync as bb

# Start with customers
c = bb.create()
c.attach("customers.csv")

# Add sales data (join name, ON expression, location)
c.join("sales", "customer_id = sale_customer_id", "sales.csv")

# Add geographic data
c.join("regions", "region = region_name", "regions.csv")

# Analyze
results = c.to_dict()
print(f"Total customers: {len(results['customer_id'])}")
```

### Streaming Large Datasets

```python
import bundlebase.sync as bb

# Open large dataset
c = bb.create().attach("100gb_dataset.parquet")

# Process in batches - memory bounded by one batch at a time
total_rows = 0
for batch in c.stream_batches():
    total_rows += batch.num_rows
    print(f"Processed {batch.num_rows} rows")

print(f"Total: {total_rows}")
```

### Saving and Loading Bundles

```python
import bundlebase.sync as bb

# Create and process
c = bb.create("/tmp/my_bundle")
c.attach("data.parquet")
c.filter("SELECT * FROM bundle WHERE year >= $1", [2020])
c.commit("Filtered to 2020+")

# Later, in another script...
c = bb.open("/tmp/my_bundle")   # read-only SyncBundle
df = c.to_pandas()

# Need to modify it? Extend first:
builder = c.extend()
builder.drop_column("internal_notes")
builder.commit("Dropped internal notes")
```

## Comparison: Async vs Sync

### Async API (for advanced use)

```python
import bundlebase
import asyncio

async def process():
    c = await bundlebase.create()
    c = await c.attach("data.parquet")
    df = await c.to_pandas()
    return df

asyncio.run(process())
```

### Sync API (simpler!)

```python
import bundlebase.sync as bb

c = bb.create()
c.attach("data.parquet")
df = c.to_pandas()
```

## Performance

### Overhead

The sync API adds minimal overhead per operation: each method call submits the underlying coroutine to a persistent `asyncio.Runner` (scripts) or the notebook's event loop via `nest_asyncio` (Jupyter). This is negligible compared to data I/O time.

### Streaming vs Materialization

For large datasets:

```python
# Good: streams data in bounded memory (one batch at a time)
for batch in c.stream_batches():
    process_batch(batch)

# Materializes: to_pandas() streams batches from Rust but accumulates
# them all to build the final DataFrame (~2x dataset size at peak)
df = c.to_pandas()
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

import bundlebase.sync as bb
```

### Display DataFrames

```python
import bundlebase.sync as bb

c = bb.create().attach("data.parquet")
display(c.to_pandas())  # Nice table in notebook
```

## Error Handling

Handle errors like any Python code. Most bundlebase errors surface as `ValueError` (see [errors.md](errors.md#python-error-mapping)):

```python
import bundlebase.sync as bb

try:
    c = bb.create()
    c.attach("nonexistent.parquet")
except ValueError as e:
    print(f"Failed to load data: {e}")

try:
    c.drop_column("nonexistent")
except ValueError as e:
    print(f"Column not found: {e}")
```

## Custom Functions

Register custom SQL functions:

```python
import bundlebase.sync as bb

# Temporary function (session-scoped, supports python runtime)
c = bb.create()
c.import_temp_function(
    name="acme.double_val",
    from_="python::my_module:double_val",
)

# Persistent function (bundled, requires serializable runtime — not python)
c.import_function(
    name="acme.weather",
    from_="ipc::weather-binary",
)
```

Temp definitions override persistent ones at resolution time. The same persistent/temp split applies to connectors (`import_connector` / `import_temp_connector`).

## Migration from Async to Sync

If you have existing async code, migration is straightforward:

### Before (Async)

```python
import bundlebase
import asyncio

async def process():
    c = await bundlebase.create()
    c = await c.attach("data.parquet")
    c = await c.filter("SELECT * FROM bundle WHERE x > $1", [10])
    return await c.to_pandas()

df = asyncio.run(process())
```

### After (Sync)

```python
import bundlebase.sync as bb

c = bb.create()
c.attach("data.parquet")
c.filter("SELECT * FROM bundle WHERE x > $1", [10])
df = c.to_pandas()
```

Just remove `await` and import `bundlebase.sync` instead of `bundlebase`!

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

And imported from bundlebase.sync:
```python
import bundlebase.sync as bb  # Not bundlebase!
```

## See Also

- [python-api.md](python-api.md) - Async API reference
- [testing.md](testing.md) - Testing strategy
- [development.md](development.md) - Development setup
