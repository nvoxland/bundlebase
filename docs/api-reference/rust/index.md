# Rust API Reference

The Bundlebase Rust API provides the core data processing functionality with high performance and memory safety.

!!! info "Rust Documentation"
    Full Rust API documentation is available in the **[Rust API Docs](../../rust/bundlebase/index.html)** (built with rustdoc).

    The Rust docs include:

    - Complete API reference for all public types
    - Detailed documentation for structs, traits, and enums
    - Implementation details and source code links
    - Examples and usage patterns
    - Trait implementations and relationships

## Key Modules

### `bundlebase`

Core bundle trait and implementations.

- **`BundleBase`** - Core trait defining bundle operations
- **`Bundle`** - Read-only bundle implementation
- **`BundleBuilder`** - Mutable bundle implementation

[View in Rust Docs](../../rust/bundlebase/index.html)

### `bundlebase::data`

Data source adapters for CSV, JSON, Parquet, and custom sources.

- **`CsvAdapter`** - CSV file support
- **`JsonAdapter`** - JSON file support
- **`ParquetAdapter`** - Parquet file support
- **`FunctionAdapter`** - Custom data generation

[View in Rust Docs](../../rust/bundlebase/data/index.html)

### `bundlebase::functions`

Custom function system for generating data programmatically.

- **`FunctionRegistry`** - Global function registry
- **`FunctionDefinition`** - Function metadata
- **`DataGenerator`** - Data generation trait

[View in Rust Docs](../../rust/bundlebase/functions/index.html)

### `bundlebase::io`

Storage abstraction layer for file systems and remote storage.

- **`StorageBackend`** - Storage trait
- **`LocalStorage`** - Local filesystem support
- **`MemoryStorage`** - In-memory storage
- **`S3Storage`** - AWS S3 support

[View in Rust Docs](../../rust/bundlebase/io/index.html)

### `bundlebase::index`

Row indexing system for efficient lookups.

- **`IndexManager`** - Index lifecycle management
- **`IndexBuilder`** - Index construction
- **`IndexQuery`** - Index-based queries

[View in Rust Docs](../../rust/bundlebase/index/index.html)

### `bundlebase::progress`

Progress tracking for long-running operations.

- **`ProgressTracker`** - Progress tracking trait
- **`ProgressScope`** - RAII progress scopes
- **`GlobalRegistry`** - Global progress registry

[View in Rust Docs](../../rust/bundlebase/progress/index.html)

### `bundlebase::versioning`

Commit and versioning system for bundles.

- **`Manifest`** - Bundle manifest
- **`CommitHistory`** - Commit tracking
- **`VersionHash`** - Content-addressed versioning

[View in Rust Docs](../../rust/bundlebase/versioning/index.html)

## For Python Users

If you're using Bundlebase from Python, you typically don't need to reference the Rust API directly. Check the **[Python API Reference](../python/index.md)** instead.

The Rust API documentation is primarily useful for:

- **Contributing** to Bundlebase development
- **Extending** Bundlebase with custom adapters
- **Understanding** implementation details
- **Debugging** complex issues

## Architecture Overview

Bundlebase uses a three-tier Rust architecture:

```
┌──────────────────┐
│  BundleBase      │  ← Core trait
│  (trait)         │
└──────────────────┘
         ▲
         │
    ┌────┴────┐
    │         │
┌───▼───┐ ┌──▼──────────┐
│Bundle │ │BundleBuilder│
│(impl) │ │(impl)       │
└───────┘ └─────────────┘
```

- **BundleBase** - Core trait defining all bundle operations
- **Bundle** - Immutable implementation for read-only access
- **BundleBuilder** - Mutable implementation for transformations

This ensures type safety and prevents accidental mutations.

## Key Technologies

- **DataFusion** - SQL query engine with Apache Arrow
- **Apache Arrow** - Columnar memory format
- **Tokio** - Async runtime
- **Serde** - Serialization framework

## Code Examples

### Using from Rust

```rust
use bundlebase::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a new bundle
    let mut builder = BundleBuilder::new("memory:///bundle")?;

    // Attach data
    builder.attach("data.parquet").await?;

    // Transform
    builder.filter("age >= 18", vec![]).await?;
    builder.remove_column("ssn").await?;

    // Query
    let df = builder.to_dataframe().await?;
    println!("Rows: {}", df.num_rows());

    // Commit
    builder.commit("Processed data").await?;

    Ok(())
}
```

### Creating Custom Adapters

```rust
use bundlebase::data::{DataAdapter, AdapterFactory};
use datafusion::prelude::*;

pub struct MyAdapter {
    // Adapter state
}

#[async_trait]
impl DataAdapter for MyAdapter {
    async fn schema(&self) -> Result<SchemaRef> {
        // Return schema
    }

    async fn execute(&self, ctx: &SessionContext) -> Result<DataFrame> {
        // Execute query
    }
}
```

## Building Rust Documentation Locally

To build the Rust documentation locally:

```bash
# Generate docs
cargo doc --no-deps --package bundlebase

# Open in browser
open target/doc/bundlebase/index.html
```

## See Also

- **[Python API](../python/index.md)** - Python bindings documentation
- **[Architecture Guide](../../guides/architecture.md)** - System architecture
- **[Development Setup](../../development/setup.md)** - Contributing to Bundlebase
- **[GitHub Repository](https://github.com/nvoxland/bundlebase)** - Source code
