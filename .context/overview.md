# Bundlebase Project Overview

## Purpose

Bundlebase is a high-performance data processing library written in Rust with Python bindings. It provides a flexible, operation-based framework for loading, transforming, and querying data from various sources using Apache Arrow and DataFusion.

## Key Technologies

- **Rust**: Core library implementation
- **DataFusion (v46)**: SQL query engine
- **Apache Arrow (v54)**: Columnar data format
- **PyO3 (v0.23)**: Python bindings
- **Tokio**: Async runtime
- **Maturin**: Python wheel builder

## Project Structure

```
├── src/
│   ├── lib.rs                          # Library entry point
│   ├── bundle.rs                       # Bundlebase trait interface
│   ├── bundle/                         # Container implementations
│   ├── data/                           # Data source plugins
│   ├── io/                             # Storage abstraction
│   ├── functions/                      # Function system
│   └── python/                         # Python bindings
├── python/Bundlebase/                  # Python package
├── tests/                              # Rust integration tests
├── test_data/                          # Test data files
├── Cargo.toml                          # Rust dependencies
└── pyproject.toml                      # Python package configuration
```

## Core Concepts at a Glance

- **Three-Tier Architecture**: Bundlebase trait, Bundlebase (read-only), BundlebaseBuilder (mutable)
- **Operation Pipeline**: Operations recorded and applied lazily during querying
- **Adapter System**: Plugin architecture for CSV, JSON, Parquet, and custom functions
- **Function System**: Custom data generation with paginated output
- **Arc-Based Sharing**: Efficient cloning with shared state
- **Manifest-Based Versioning**: Commit history with 'from' chain support


## Quick Start

### Basic Python Usage

```python
import bundlebase

# Create a new container
c = await bundlebase.create("/path/to/container")

# Attach data
await c.attach("data.parquet")

# Transform data (mutations are in-place)
await c.remove_column("unwanted")
await c.filter("active = true")

# Export results
df = await c.to_pandas()

# Commit changes
await c.commit("Data transformation")
```

### Building and Testing

```bash
# Setup
poetry install

# Build Rust extension
maturin develop

# Run tests
cargo test                # Rust tests
poetry run pytest         # Python tests
```

## Current State

**Major Features Implemented:**
- ✅ Three-tier architecture with proper immutability semantics
- ✅ Commit-based versioning with manifest history
- ✅ 'From' chain support for container branching
- ✅ Mutable operations pattern (all operations mutate in place)
- ✅ Three-phase operation system (check/reconfigure/apply)
- ✅ Flexible path handling (file://, memory:///, s3://, custom URLs)
- ✅ Shared FunctionRegistry across container instances
- ✅ Python function integration via PyO3
- ✅ Multi-format file support (CSV, JSON, Parquet)
- ✅ Custom data generation via function:// URLs
- ✅ Row indexing system for efficient lookups
- ✅ Lazy evaluation with DataFusion integration
- ✅ Schema tracking through operation pipeline
- ✅ Proper error propagation to Python
- ✅ Async/await support for Python bindings
- ✅ Schema introspection with PySchema, PySchemaField

**Known Limitations:**
- ❌ Limited input validation (some operations don't validate preconditions)
- ❌ Schema mismatch handling in multi-source UNIONs could be improved
- ⚠️ Row indexing is lazy and built on first use (not pre-computed)

## Development Guidelines

- Always start with Rust code and ensure it's working and well tested, THEN write Python code
- PyO3 code should only be used in the python module
- Don't create mod.rs files
- The project is not launched, so never need to keep methods purely for compatibility reasons
- Python tests should be E2E tests focusing on the Python binding, not testing underlying business logic
- Only do a git commit when explicitly told to
