# Bundlebase

A versioned dataset that carries its own context, so an AI agent can query it correctly and every number can be traced back to its source.

Hand an agent a CSV and it writes SQL that looks right. A schema says a column is called `amt` and holds a number; it doesn't say the value is net of refunds, or what your company means by "revenue". A bundle carries that context with the data: what each column means, how your metrics are defined, where every row came from, and every transformation applied. Serve it over MCP and an agent reads all of it in one call.

The same bundle works for people — Python, the CLI, or any BI tool over Arrow Flight SQL.

**[Documentation](https://nvoxland.github.io/bundlebase/)** |
[PyPI](https://pypi.org/project/bundlebase/) |
[Issues](https://github.com/nvoxland/bundlebase/issues)

## Features

- **Provenance**: Every commit records who changed what, when, and how. Every attached file keeps its source and checksum, so any number can be traced back
- **Semantic model**: Column descriptions and named metrics travel in the bundle, so everyone (and every agent) computes a number the same way
- **Agent-native**: MCP server with `describe`, `analyze`, `data_quality`, and `metric` tools. `bundlebase setup-agent` writes the config
- **Version history**: Commit, undo, and reset. History is append-only and never rewritten
- **Zero infrastructure**: A bundle is a directory of YAML manifests and Parquet files, on local disk, S3, GCS, or Azure. No server, no catalog
- **Works with your tools**: Python (pandas, polars, pyarrow), a CLI, and BI tools over Arrow Flight SQL
- **Many sources**: Parquet, CSV, and JSONL from local files, S3, HTTP, SFTP, FTP, Kaggle, or PostgreSQL
- **Streaming**: Rust core on Apache Arrow and DataFusion. Datasets larger than RAM, constant memory

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
