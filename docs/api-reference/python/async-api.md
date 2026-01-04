# Async API

The async API provides a modern async/await interface for Bundlebase operations.

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
        - extend
        - schema
        - name
        - description
        - num_rows
        - history
        - to_pandas
        - to_polars
        - to_numpy
        - to_dict
        - explain

### PyBundleBuilder

Mutable bundle class returned by `create()` and transformation methods.

::: bundlebase.PyBundleBuilder
    options:
      show_root_heading: true
      show_root_full_path: false
      members:
        - attach
        - remove_column
        - rename_column
        - filter
        - select
        - join
        - attach_to_join
        - set_name
        - set_description
        - define_function
        - create_index
        - rebuild_index
        - reindex
        - commit
        - schema
        - name
        - description
        - num_rows
        - to_pandas
        - to_polars
        - to_numpy
        - to_dict
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
c = await c.filter("age >= 18")
c = await c.remove_column("ssn")

# Export
df = await c.to_pandas()
```

### Method Chaining

```python
import bundlebase

c = await (bundlebase.create()
    .attach("data.parquet")
    .filter("active = true")
    .remove_column("temp")
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
c = await c.filter("year >= 2020")

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
