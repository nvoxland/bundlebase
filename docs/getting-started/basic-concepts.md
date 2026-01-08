# Basic Concepts

Understanding these core concepts will help you get the most out of Bundlebase.

## What is a Bundle?

A **bundle** is like a container for your data. Think of it as:

- A collection of data from one or more sources
- A series of transformations to apply to that data
- A snapshot that can be versioned and saved

```python
import bundlebase as bb

# Create a new bundle
c = await bb.create("my/path")

# Add data to it
c = await c.attach("data.parquet")

# Transform it
c = await c.filter("age >= 18")

# Export it
df = await c.to_pandas()
```

## Read-Only vs Mutable Bundles

Bundlebase has two types of bundles:

### PyBundle (Read-Only)

When you open an existing bundle, you get a **read-only** bundle:

```python
# Opening returns PyBundle (read-only)
c = await bb.open("/path/to/bundle")

# You can read data
df = await c.to_pandas()
schema = c.schema
rows = c.num_rows

# But you can extend it to create a mutable copy
c = await c.extend()  # Now it's mutable
await c.filter("active = true")
```

### PyBundleBuilder (Mutable)

When you create a new bundle or extend an existing one, you get a **mutable** bundle that you can transform:

```python
# Creating returns PyBundleBuilder (mutable)
c = await bb.create("my/bundle")

# You can transform it
await c.attach("data.parquet")
await c.filter("age >= 18")
await c.remove_column("ssn")
```

!!! tip "All Mutations Are In-Place"
    Bundlebase mutates in place - methods like `filter()` and `attach()` modify the bundle and return `self` for chaining. This is different from pandas where operations return new copies.

## Operation Pipeline

Bundlebase uses **lazy evaluation** - operations are recorded but not executed immediately:

```python
# These operations are just recorded, not executed yet
c = await bb.create("my/path")
c = await c.attach("data.parquet")
c = await c.filter("age >= 18")
c = await c.remove_column("ssn")

# Execution happens here when you export
df = await c.to_pandas()  # Now the pipeline executes
```

This allows Bundlebase to:

- Optimize the entire pipeline before execution
- Push filters down to the data source
- Avoid loading unnecessary columns
- Stream data instead of loading everything into memory

## Data Sources and Formats

Bundlebase supports multiple data formats:

```python
# Parquet files (fast, columnar)
c = await c.attach("data.parquet")

# CSV files
c = await c.attach("data.csv")

# JSON files
c = await c.attach("data.json")

# Multiple sources (will be unioned)
c = await c.attach("january.parquet")
c = await c.attach("february.parquet")
c = await c.attach("march.parquet")
```

## Versioning and Commits

Bundlebase includes built-in version control similar to Git:

```python
# Create a bundle at a specific path
c = await bb.create("/path/to/bundle")

# Make changes
c = await c.attach("data.parquet")
c = await c.filter("year >= 2020")

# Commit your changes
await c.commit("Initial data load with 2020+ filter")

# Later, open the saved bundle
c = await bb.open("/path/to/bundle")

# View commit history
history = c.history()
for commit in history:
    print(f"{commit.version}: {commit.message}")
```

## Indexing

Indexes enable fast lookups on specific columns:

```python
# Define an index on a column
c = await c.create_index("email")

# Now queries on email will be faster
c = await c.filter("email = 'user@example.com'")

# Rebuild an index if data changes
c = await c.rebuild_index("email")
```

Bundlebase uses a sophisticated indexing system that:

- Builds indexes lazily (only when needed)
- Uses cost-based optimization to decide when to use indexes
- Supports multiple index types for different data types

Learn more in the [Indexing Guide](../guide/indexing.md).
