# Bundlebase

Like Docker, but for data.

**[Documentation](https://nvoxland.github.io/bundlebase/)** |
[PyPI](https://pypi.org/project/bundlebase/) |
[Issues](https://github.com/nvoxland/bundlebase/issues)

## Features

- **Multiple Formats**: Support for Parquet, CSV, JSON, and more
- **Version Control**: Built-in commit system for data pipeline versioning
- **Python Native**: Seamless async/sync Python API with type hints
- **High Performance**: Rust-powered core with Apache Arrow columnar format
- **Fluent API**: Chain operations with intuitive, readable syntax

## Installation

```bash
pip install bundlebase
```

## Quick Start

### Async API

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

### Sync API

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

## Streaming Large Datasets

Process data larger than RAM efficiently:

```python
import bundlebase

# Stream batches instead of loading everything
c = await bundlebase.open("huge_dataset.parquet")

total_rows = 0
async for batch in bundlebase.stream_batches(c):
    # Each batch is ~100MB, not entire dataset
    total_rows += batch.num_rows
    # Memory is freed after each iteration

print(f"Processed {total_rows} rows")
```

## Core Operations

### Data Loading
```python
c = await bundlebase.create()
c = c.attach("data.parquet")      # Parquet files
c = c.attach("data.csv")          # CSV files
c = c.attach("data.json")         # JSON files
```

### Data Transformation
```python
c = c.filter("active = true")              # Filter rows
c = c.select(["id", "name", "email"])      # Select columns
c = c.remove_column("temp_field")          # Remove columns
c = c.rename_column("old", "new")          # Rename columns
c = c.select("SELECT * FROM self WHERE ...") # SQL queries
```

### Data Export
```python
df = await c.to_pandas()    # → pandas DataFrame
df = await c.to_polars()    # → polars DataFrame
arr = await c.to_numpy()    # → NumPy array
data = await c.to_dict()    # → Python dict
```

### Indexing
```python
c = c.create_index("email")        # Create index for fast lookups
c = c.rebuild_index("email")       # Rebuild existing index
```

### Joining
```python
c = await bundlebase.create()
c = c.attach("customers.parquet")
c = c.join(
    "orders.parquet",
    left_on="customer_id",
    right_on="id",
    join_type="inner"
)
```

## Development

### Prerequisites
- Rust (latest stable)
- Python 3.9+
- Poetry

### Setup

```bash
# Install Python dependencies
poetry install

# Build Rust extension
maturin develop

# Run tests
cargo test              # Rust tests
poetry run pytest       # Python tests
```

## Contributing

Contributions are welcome! 

## License

Distributed under the Apache 2.0 license.
