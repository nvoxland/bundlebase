# Bundlebase Project Overview

Bundlebase is a high-performance data processing library written in Rust with Python bindings. It provides a flexible, operation-based framework for loading, transforming, and querying data from various sources using Apache Arrow and DataFusion.

## Documentation

For detailed documentation, see the modular guides in the `claude/` directory:

- **[00-OVERVIEW.md](claude/00-OVERVIEW.md)** - Project purpose, structure, and development guidelines
- **[01-ARCHITECTURE.md](claude/01-ARCHITECTURE.md)** - Three-tier architecture, operations, adapters, function system
- **[02-PYTHON-API.md](claude/02-PYTHON-API.md)** - Complete Python API reference with examples
- **[03-PYTHON-BINDINGS.md](claude/03-PYTHON-BINDINGS.md)** - PyO3 integration and async bridge
- **[04-VERSIONING.md](claude/04-VERSIONING.md)** - Commit-based versioning and path handling
- **[05-TESTING.md](claude/05-TESTING.md)** - Testing strategy and execution
- **[06-DEVELOPMENT.md](claude/06-DEVELOPMENT.md)** - Setup, build, and development workflow
- **[07-ROW-INDEXING.md](claude/07-ROW-INDEXING.md)** - Row indexing system
- **[08-PROGRESS-TRACKING.md](claude/08-PROGRESS-TRACKING.md)** - Progress tracking for long-running operations

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

## Key Principles

- **Always start with Rust**, then write Python bindings
- **All operations mutate in place** - modification methods return `&mut self`
- **Lazy evaluation** - queries execute on demand, not when operations are recorded
- **Streaming execution** - use `execute_stream()`, never `collect()` for memory efficiency
- **Three phases** - operations validate, reconfigure state, then apply to DataFrames
- **Shared state via Arc** - cheap cloning with efficient reference counting
- **E2E Python tests** - test the Python binding, not underlying Rust logic

See [claude/06-DEVELOPMENT.md](claude/06-DEVELOPMENT.md) for full development workflow.

## Performance Guidelines

**Memory-Efficient Data Processing:**

Bundlebase uses streaming execution throughout to handle datasets larger than RAM:

- ✅ **Python**: Use `to_pandas()` / `to_polars()` - they stream internally (constant memory)
- ✅ **Python**: Use `stream_batches()` for custom incremental processing
- ✅ **Rust**: Always use `execute_stream()` for query execution
- ❌ **Rust**: Never use `collect()` - materializes entire dataset in memory (3x size)
- ❌ **Python**: Avoid `as_pyarrow()` for large datasets - use `stream_batches()` instead

**See:**
- [claude/01-ARCHITECTURE.md](claude/01-ARCHITECTURE.md#streaming-execution-architecture) for architectural details
- [claude/02-PYTHON-API.md](claude/02-PYTHON-API.md#streaming-api-for-large-datasets) for Python streaming API
