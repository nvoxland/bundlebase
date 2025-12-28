# Basic Concepts

Understanding these core concepts will help you get the most out of Bundlebase.

## What is a Bundle?

A **bundle** is like a container for your data. Think of it as:

- A collection of data from one or more sources
- A series of transformations to apply to that data
- A snapshot that can be versioned and saved

```python
import bundlebase

# Create a new bundle
c = await bundlebase.create()

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
c = await bundlebase.open("/path/to/bundle")

# You can read data
df = await c.to_pandas()
schema = c.schema
rows = c.num_rows

# But you can extend it to create a mutable copy
c = await c.extend()  # Now it's mutable
c = await c.filter("active = true")
```

### PyBundleBuilder (Mutable)

When you create a new bundle, you get a **mutable** bundle that you can transform:

```python
# Creating returns PyBundleBuilder (mutable)
c = await bundlebase.create()

# You can transform it
c = await c.attach("data.parquet")
c = await c.filter("age >= 18")
c = await c.remove_column("ssn")
```

!!! tip "All Mutations Are In-Place"
    Bundlebase mutates in place - methods like `filter()` and `attach()` modify the bundle and return `self` for chaining. This is different from pandas where operations return new copies.

## Operation Pipeline

Bundlebase uses **lazy evaluation** - operations are recorded but not executed immediately:

```python
# These operations are just recorded, not executed yet
c = await bundlebase.create()
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

## Streaming Execution

For datasets larger than RAM, use **streaming**:

```python
# Don't do this for large datasets:
# df = await c.to_pandas()  # Loads entire dataset into memory!

# Instead, stream batches:
async for batch in bundlebase.stream_batches(c):
    # Process each batch (typically ~100MB)
    # Memory is freed after each iteration
    process_batch(batch)
```

!!! warning "Memory Efficiency"
    Always use `to_pandas()` / `to_polars()` (which stream internally) or `stream_batches()` for custom processing. Never use `as_pyarrow()` for large datasets.

## Versioning and Commits

Bundlebase includes built-in version control similar to Git:

```python
# Create a bundle at a specific path
c = await bundlebase.create("/path/to/bundle")

# Make changes
c = await c.attach("data.parquet")
c = await c.filter("year >= 2020")

# Commit your changes
await c.commit("Initial data load with 2020+ filter")

# Later, open the saved bundle
c = await bundlebase.open("/path/to/bundle")

# View commit history
history = c.history()
for commit in history:
    print(f"{commit.version}: {commit.message}")
```

## Indexing

Indexes enable fast lookups on specific columns:

```python
# Define an index on a column
c = await c.define_index("email")

# Now queries on email will be faster
c = await c.filter("email = 'user@example.com'")

# Rebuild an index if data changes
c = await c.rebuild_index("email")
```

Bundlebase uses a sophisticated indexing system that:

- Builds indexes lazily (only when needed)
- Uses cost-based optimization to decide when to use indexes
- Supports multiple index types for different data types

Learn more in the [Row Indexing Guide](../guides/row-indexing.md).

## Joins

Combine data from multiple sources:

```python
# Start with one dataset
c = await bundlebase.create()
c = await c.attach("customers.parquet")

# Join with another dataset
c = await c.join(
    "orders.parquet",
    left_on="customer_id",
    right_on="id",
    join_type="inner"  # or "left", "right", "outer"
)

# The result includes columns from both datasets
df = await c.to_pandas()
```

## Views

Views are named snapshots of your data pipeline:

```python
# Create a view
c = await c.views().create("active_users", "SELECT * FROM self WHERE active = true")

# Later, query the view
df = await c.views().query("active_users")

# Views are versioned with commits
await c.commit("Added active_users view")
```

Learn more in the [Views Guide](../guides/views.md).

## Progress Tracking

For long-running operations, monitor progress:

```python
from bundlebase import StreamProgress

# Create a progress tracker
progress = StreamProgress()

# Use it during operations
c = await bundlebase.create()
async for batch in bundlebase.stream_batches(c, progress=progress):
    print(f"Progress: {progress.percentage:.1f}%")
    process_batch(batch)
```

Learn more in the [Progress Tracking Guide](../guides/progress-tracking.md).

## Architecture Overview

Bundlebase has a three-tier architecture:

1. **BundleBase Trait** - Core interface defining what a bundle can do
2. **Bundle** - Read-only implementation (`PyBundle` in Python)
3. **BundleBuilder** - Mutable implementation (`PyBundleBuilder` in Python)

```
┌─────────────────────────┐
│    BundleBase Trait     │  ← Core interface
└─────────────────────────┘
            ▲
            │
      ┌─────┴─────┐
      │           │
┌─────▼─────┐ ┌──▼──────────┐
│   Bundle   │ │ BundleBuilder │
│ (read-only)│ │  (mutable)    │
└────────────┘ └───────────────┘
      ▲              ▲
      │              │
┌─────┴─────┐ ┌─────┴──────────┐
│  PyBundle │ │ PyBundleBuilder │  ← Python bindings
└───────────┘ └────────────────┘
```

This architecture ensures:

- **Immutability**: Opened bundles can't be accidentally modified
- **Type Safety**: Rust's type system prevents invalid operations
- **Flexibility**: Easy to extend with new operations

For a deep dive, see the [Architecture Guide](../guides/architecture.md).

## Technology Stack

Bundlebase is built on industry-leading technologies:

- **Rust** - Memory-safe, high-performance core
- **Apache Arrow** - Columnar data format optimized for analytics
- **DataFusion** - SQL query engine with advanced optimizations
- **PyO3** - Seamless Python bindings
- **Tokio** - Async runtime for concurrent operations

## Key Principles

### 1. Lazy Evaluation

Operations are planned, not executed:

```python
# These don't load any data:
c = await c.attach("data.parquet")
c = await c.filter("age >= 18")

# This triggers execution:
df = await c.to_pandas()
```

### 2. In-Place Mutation

Methods modify the bundle and return `self`:

```python
# Same object after each operation
c = await bundlebase.create()
id_before = id(c)

c = await c.attach("data.parquet")
assert id(c) == id_before  # Same object!
```

### 3. Streaming by Default

Bundlebase streams data to handle datasets larger than RAM:

```python
# Internally streams, constant memory
df = await c.to_pandas()  # ✓ Good

# Loads everything into memory
all_data = await c.as_pyarrow()  # ✗ Avoid for large datasets
```

### 4. Type Safety

Rust's type system prevents many errors at compile time:

- Invalid column names caught early
- Type mismatches in filters detected
- Schema incompatibilities surfaced immediately

## Next Steps

Now that you understand the basics:

- **[Architecture Guide](../guides/architecture.md)** - Deep dive into how Bundlebase works
- **[API Reference](../api-reference/python/index.md)** - Complete API documentation
- **[Examples](../examples/basic-operations.md)** - Practical code examples
- **[Guides](../guides/versioning.md)** - Advanced topics like versioning, indexing, and views
