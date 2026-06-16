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
    .filter("SELECT * FROM bundle WHERE age >= 18")
    .drop_column("ssn")
    .rename_column("fname", "first_name"))

# Convert to pandas
df = await c.to_pandas()

# Commit changes
await c.commit("Cleaned customer data")
```

### Sync API

```python
import bundlebase.sync as bb

# Same operations, no await needed
c = (bb.create()
    .attach("data.parquet")
    .filter("SELECT * FROM bundle WHERE age >= 18")
    .drop_column("ssn")
    .rename_column("fname", "first_name"))

df = c.to_pandas()
c.commit("Cleaned customer data")
```

## Streaming Large Datasets

Process data larger than RAM efficiently:

```python
import bundlebase

# Stream batches instead of loading everything
c = await bundlebase.create().attach("huge_dataset.parquet")

total_rows = 0
async for batch in bundlebase.stream_batches(c):
    # Each batch is a slice of the data, not the entire dataset
    total_rows += batch.num_rows
    # Memory is freed after each iteration

print(f"Processed {total_rows} rows")
```

## Core Operations

### Data Loading
```python
c = await bundlebase.create()
c = await c.attach("data.parquet")      # Parquet files
c = await c.attach("data.csv")          # CSV files
c = await c.attach("data.jsonl")        # JSON Lines files
```

### Data Transformation
```python
c = await c.filter("SELECT * FROM bundle WHERE active = true")  # Filter rows
c = await c.drop_column("temp_field")                           # Remove columns
c = await c.rename_column("old", "new")                         # Rename columns

rs = await c.query("SELECT * FROM bundle WHERE id > 10")        # SQL queries
df = await rs.to_pandas()
```

### Data Export
```python
df = await c.to_pandas()    # → pandas DataFrame
df = await c.to_polars()    # → polars DataFrame
arr = await c.to_numpy()    # → dict of NumPy arrays
data = await c.to_dict()    # → Python dict
```

### Indexing
```python
c = await c.create_index("email", "btree")   # Create index for fast lookups
c = await c.rebuild_index("email")           # Rebuild existing index
```

### Joining
```python
c = await bundlebase.create()
c = await c.attach("customers.parquet")
c = await c.join(
    "orders",
    "bundle.customer_id = orders.id",
    location="orders.parquet",
    join_type="inner",
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
./scripts/maturin-dev.sh

# Run tests
cargo test --profile test       # Rust tests
poetry run pytest python/tests/ # Python tests
```

## Contributing

Contributions are welcome! 

## License

Distributed under the Apache 2.0 license.
