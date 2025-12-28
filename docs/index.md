# Bundlebase

*Like Docker, but for data.*

Bundlebase is a high-performance data processing library written in Rust with Python bindings. It provides a flexible, operation-based framework for loading, transforming, and querying data from various sources using Apache Arrow and DataFusion.

## Features

- **Multiple Formats** - Support for Parquet, CSV, JSON, and more
- **Version Control** - Built-in commit system for data pipeline versioning
- **Python Native** - Seamless async/sync Python API with type hints
- **High Performance** - Rust-powered core with Apache Arrow columnar format
- **Fluent API** - Chain operations with intuitive, readable syntax

## Quick Example

=== "Async API"

    ```python
    import bundlebase

    # Create a new bundle and chain operations
    c = await (bundlebase.create()
        .attach("data.parquet")
        .filter("age >= 18")
        .remove_column("ssn")
        .rename_column("fname", "first_name"))

    # Convert to pandas
    df = await c.to_pandas()

    # Commit changes
    await c.commit("Cleaned customer data")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as dc

    # Same operations, no await needed
    c = (dc.create()
        .attach("data.parquet")
        .filter("age >= 18")
        .remove_column("ssn")
        .rename_column("fname", "first_name"))

    df = c.to_pandas()
    c.commit("Cleaned customer data")
    ```

## Getting Started

<div class="grid cards" markdown>

-   __[Installation](getting-started/installation.md)__

    ---

    Install Bundlebase and get up and running quickly

-   __[Quick Start](getting-started/quick-start.md)__

    ---

    Learn the basics with hands-on examples

-   __[Basic Concepts](getting-started/basic-concepts.md)__

    ---

    Understand core concepts and architecture

-   __[API Reference](api-reference/python/index.md)__

    ---

    Complete Python and Rust API documentation

</div>

## Why Bundlebase?

Bundlebase combines the best of data engineering and software engineering:

- **Memory-Efficient Streaming**: Process datasets larger than RAM using streaming execution
- **Type-Safe Operations**: Leverage Rust's type system for reliable data transformations
- **Git-Like Versioning**: Track changes to your data pipelines with commits and history
- **Flexible Deployment**: Run in Jupyter notebooks, Python scripts, or production pipelines

## Core Operations

### Loading Data

```python
import bundlebase

c = await bundlebase.create()
c = await c.attach("data.parquet")      # Parquet files
c = await c.attach("data.csv")          # CSV files
c = await c.attach("data.json")         # JSON files
```

### Transforming Data

```python
c = await c.filter("active = true")              # Filter rows
c = await c.select(["id", "name", "email"])      # Select columns
c = await c.remove_column("temp_field")          # Remove columns
c = await c.rename_column("old", "new")          # Rename columns
c = await c.select("SELECT * FROM self WHERE ...") # SQL queries
```

### Exporting Results

```python
df = await c.to_pandas()    # → pandas DataFrame
df = await c.to_polars()    # → polars DataFrame
arr = await c.to_numpy()    # → NumPy array
data = await c.to_dict()    # → Python dict
```

### Streaming Large Datasets

Process data larger than RAM efficiently:

```python
# Stream batches instead of loading everything
c = await bundlebase.open("huge_dataset.parquet")

total_rows = 0
async for batch in bundlebase.stream_batches(c):
    # Each batch is ~100MB, not entire dataset
    total_rows += batch.num_rows
    # Memory is freed after each iteration

print(f"Processed {total_rows} rows")
```

## Learn More

<div class="grid cards" markdown>

-   __[Guides](guides/architecture.md)__

    ---

    Deep dive into architecture, versioning, indexing, and more

-   __[Examples](examples/basic-operations.md)__

    ---

    Practical examples and use cases

-   __[Development](development/setup.md)__

    ---

    Contribute to Bundlebase development

</div>

[Get Started](getting-started/installation.md){ .md-button .md-button--primary }
[View on GitHub](https://github.com/nvoxland/bundlebase){ .md-button }
