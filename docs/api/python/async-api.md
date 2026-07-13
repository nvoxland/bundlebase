---
seo_title: "Python Async API Reference - Bundlebase"
---

# Async API

The async API provides an async/await interface for Bundlebase operations.

## Factory Functions

### create

::: bundlebase.create
    options:
      show_root_heading: true
      show_root_full_path: false

### open

::: bundlebase.open
    options:
      show_root_heading: true
      show_root_full_path: false

## Core Classes

### PyBundle

Read-only bundle class returned by `open()`.

::: bundlebase.PyBundle
    options:
      show_root_heading: true
      show_root_full_path: false
      members:
        - query
        - extend
        - import_temp_connector
        - drop_temp_connector
        - schema
        - name
        - description
        - id
        - url
        - version
        - num_rows
        - history
        - operations
        - view
        - views
        - verify_data
        - describe_data
        - export_tar
        - to_pandas
        - to_polars
        - to_numpy
        - to_dict
        - as_pyarrow_stream
        - explain

### PyBundleBuilder

Mutable bundle class returned by `create()` and transformation methods.

::: bundlebase.PyBundleBuilder
    options:
      show_root_heading: true
      show_root_full_path: false
      members:
        - query
        - attach
        - detach_block
        - add_column
        - cast_column
        - drop_column
        - rename_column
        - normalize_column_names
        - describe_column
        - define_metric
        - update
        - delete
        - always_delete
        - drop_always_delete
        - always_update
        - drop_always_update
        - filter
        - join
        - drop_join
        - create_view
        - drop_view
        - set_name
        - set_description
        - import_function
        - drop_function
        - create_source
        - import_connector
        - import_temp_connector
        - drop_connector
        - drop_temp_connector
        - fetch
        - fetch_all
        - create_index
        - rebuild_index
        - drop_index
        - reindex
        - undo
        - reset
        - commit
        - extend
        - export_tar
        - export_empty
        - schema
        - name
        - description
        - id
        - url
        - version
        - num_rows
        - status
        - history
        - operations
        - view
        - views
        - verify_data
        - describe_data
        - to_pandas
        - to_polars
        - to_numpy
        - to_dict
        - as_pyarrow_stream
        - explain

## Supporting Classes

### PyBundleStatus

::: bundlebase.PyBundleStatus
    options:
      show_root_heading: true
      show_root_full_path: false

### PyChange

::: bundlebase.PyChange
    options:
      show_root_heading: true
      show_root_full_path: false

## Utility Functions

### set_rust_log_level

::: bundlebase.set_rust_log_level
    options:
      show_root_heading: true
      show_root_full_path: false

### test_datafile

::: bundlebase.test_datafile
    options:
      show_root_heading: true
      show_root_full_path: false

### random_memory_url

::: bundlebase.random_memory_url
    options:
      show_root_heading: true
      show_root_full_path: false

## Examples

### Basic Usage

```python
import bundlebase

# Create a new bundle
c = await bundlebase.create()

# Attach data
c = await c.attach("data.parquet")

# Transform
c = await c.filter("SELECT * FROM bundle WHERE age >= 18")
c = await c.drop_column("ssn")

# Export
df = await c.to_pandas()
```

### Method Chaining

```python
import bundlebase

c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("SELECT * FROM bundle WHERE active = true")
    .drop_column("temp")
    .rename_column("old", "new"))

df = await c.to_pandas()
```

### Opening Saved Bundles

```python
import bundlebase

# Open existing bundle
c = await bundlebase.open("/path/to/bundle")

# Extend it (creates mutable copy)
c = await c.extend()

# Add more operations
c = await c.filter("SELECT * FROM bundle WHERE year >= 2020")

# Commit changes
await c.commit("Filtered to 2020+")
```

### Error Handling

```python
import bundlebase

try:
    c = await bundlebase.create()
    c = await c.attach("nonexistent.parquet")
except ValueError as e:
    print(f"Error loading data: {e}")
```

## See Also

- **[Sync API](sync-api.md)** - Synchronous interface
- **[Conversion Functions](conversion.md)** - Data export utilities
- **[Operation Chains](chain.md)** - Fluent chaining implementation
