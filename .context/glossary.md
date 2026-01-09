# Bundlebase Glossary

This document defines project-specific terminology used throughout the Bundlebase codebase and documentation. Use these exact terms for consistency.

## Core Concepts

### Bundle (Trait)

**Definition**: The common interface shared by all container types in Bundlebase.

**Usage**: Defines methods for schema introspection, querying, and data conversion that work across both mutable and immutable containers.

**Implemented By**: `Bundlebase` (read-only) and `BundlebaseBuilder` (mutable)

**Location**: `rust/bundlebase/src/bundle.rs`

**See Also**: [architecture.md](architecture.md)

### Bundlebase (Read-Only Container)

**Definition**: An immutable container representing a committed snapshot of data.

**Characteristics**:
- Loaded from versioned manifests in `{data_dir}/_bundlebase/`
- Cannot be modified directly
- Thread-safe due to immutability
- Can be extended via `extend(new_data_dir)` to create a `BundleBuilder`

**Python Type**: `Bundle` (read-only wrapper)

**Usage Example**:
```python
# Load committed container
c = await bundlebase.open("/path/to/container")
df = await c.to_pandas()  # Can read, cannot modify
```

**See Also**: [architecture.md](architecture.md)

### BundleBuilder (Mutable Container)

**Definition**: A mutable container for building and transforming data.

**Characteristics**:
- Wraps a `Bundlebase` with a working directory
- Tracks new operations applied since the base container
- All modification methods mutate in-place and return `&mut self`
- Can be committed via `commit(message)` to create a versioned snapshot

**Methods**: `attach()`, `remove_column()`, `rename_column()`, `filter()`, `select()`, `join()`, `define_function()`, `commit()`

**Python Type**: `BundleBuilder` (mutable wrapper)

**Usage Example**:
```python
# Create mutable container
c = await bundlebase.create("/path/to/container")
c = c.attach("data.parquet").filter("age >= 18")  # Chainable mutations
await c.commit("Filtered adults")
```

**See Also**: [architecture.md](architecture.md), [python-api.md](python-api.md)

### BundleState

**Definition**: Shared internal state used by both `Bundlebase` and `BundleBuilder`.

**Contains**:
- Arrow `SchemaRef` (schema definition)
- Metadata (name, description)
- Row count tracking
- SessionContext for DataFusion
- FunctionRegistry (shared across all containers)
- Adapter factory

**Sharing**: Uses `Arc` for efficient cloning - all clones share the same state

**See Also**: [architecture.md](architecture.md)

## Operations

### Operation

**Definition**: A recorded transformation step in the data pipeline.

**Types**:
- **AttachBlock**: Add data from sources (union operation)
- **RemoveColumns**: Filter out columns
- **RenameColumn**: Rename columns
- **Filter**: Filter rows based on SQL predicates
- **Select**: Select specific columns or execute SQL
- **Join**: Join with other data sources
- **Query**: Execute custom SQL queries
- **DefineFunction**: Register custom data generation functions
- **SetName**: Set container name
- **SetDescription**: Set container description
- **IndexData**: Track row indexing metadata

**Lifecycle**:
1. **Recorded** when operation method is called
2. **Validated** immediately via `check()`
3. **Applied to schema** immediately via `reconfigure()`
4. **Executed on DataFrame** lazily during query via `apply_dataframe()`

**See Also**: [architecture.md](architecture.md)

### Lazy Evaluation

**Definition**: Pattern where operations are recorded when called but only executed during query time.

**Example**:
```python
c = c.filter("age >= 18")    # RECORDED only
c = c.select(["name"])       # RECORDED only
df = await c.to_pandas()     # NOW both operations EXECUTE
```

**Benefit**: Enables query optimization and composable transformations

**See Also**: [ai-rules.md](ai-rules.md)

### Adapter

**Definition**: Plugin component that handles loading data from specific source types.

**Built-in Adapters**:
- **CsvPlugin**: CSV file support
- **JsonPlugin**: Line-delimited JSON support
- **ParquetPlugin**: Apache Parquet support
- **FunctionPlugin**: Custom function data sources via `function://` URLs

**Location**: `rust/bundlebase/src/data/`

**See Also**: [architecture.md](architecture.md)

### FunctionRegistry

**Definition**: Shared registry (via `Arc<RwLock>`) that stores all function definitions and implementations.

**Characteristics**:
- Global across all container instances
- Thread-safe with read-write locking
- Stores both signatures and implementations
- Enables function reuse across containers

**Methods**:
- `register(signature)` - Add function definition
- `set_impl(name, impl)` - Set function implementation
- `get(name)` - Retrieve function

**See Also**: [architecture.md](architecture.md)

### DataGenerator

**Definition**: Trait for paginated data generation used by custom functions.

**Interface**:
```rust
pub trait DataGenerator {
    fn next(&mut self, page: u64) -> Result<Option<RecordBatch>>;
}
```

**Usage**: Called repeatedly with incrementing page numbers (0, 1, 2...) until returns `None`.

**Implementation**: Custom data sources implement this trait to provide streaming data

**See Also**: [architecture.md](architecture.md), [python-api.md](python-api.md)

## Versioning

### Commit

**Definition**: A versioned snapshot of a container stored as a YAML manifest.

**Storage**: `{data_dir}/_bundlebase/{version}-{hash}.yaml`

**Contains**:
- Container metadata (name, description, schema, row count)
- Base container reference (`from` field)
- List of operations applied
- Commit metadata (version number, hash, timestamp, message)

**Example Filename**: `00001-a1b2c3d4e5f6.yaml`

**See Also**: [versioning.md](versioning.md)

### Manifest

**Definition**: The YAML file format used to persist container commits.

**Format**:
```yaml
name: "container_name"
description: "Description"
num_rows: 1000
schema:
  - name: "id"
    type: "Int64"
from: null  # or path to base container
operations:
  - type: "AttachBlock"
    source: "data.parquet"
version: 1
hash: "a1b2c3d4e5f6"
created_at: "2024-01-15T10:30:00Z"
message: "Commit message"
```

**Location**: `{data_dir}/_bundlebase/`

**See Also**: [versioning.md](versioning.md)

### Version Hash

**Definition**: A 12-character hash used to uniquely identify a commit.

**Format**: Lowercase hexadecimal (e.g., `a1b2c3d4e5f6`)

**Purpose**: Ensures commit uniqueness and enables content-addressable storage

**See Also**: [versioning.md](versioning.md)

### From Chain

**Definition**: Container inheritance mechanism where a container extends another container.

**Usage**:
```python
base = await bundlebase.open("/base/container")
extended = await base.extend("/extended/container")
```

**Manifest Field**: `from: "/base/container"` or `from: "../"` (relative path)

**Benefits**:
- Version history preservation
- Branching support
- Relative path portability
- Circular dependency detection

**See Also**: [versioning.md](versioning.md)

## Indexing

### Row Index / Column Index

**Definition**: Data structure that accelerates queries with equality, IN, and range predicates.

**Storage**: Binary files in `{data_dir}/_bundlebase/indexes/`

**Structure**:
```rust
pub struct ColumnIndex {
    column_name: String,
    data_type: DataType,
    blocks: Vec<IndexBlock>,       // Value -> RowId mappings
    directory: IndexDirectory,      // Block-level min/max
    total_entries: u64,
    total_rows: u64,
}
```

**Python API**:
- `create_index(column_name)` - Create index on column
- `rebuild_index(column_name)` - Rebuild existing index

**See Also**: [indexing.md](indexing.md)

### RowId

**Definition**: Unique identifier for a row combining block ID and offset.

**Structure**:
```rust
pub struct RowId {
    block_id: ObjectId,  // Identifies data block
    offset: u64,         // Offset within block (0-based)
}
```

**Usage**: Internal tracking for row identification across blocks

**See Also**: [indexing.md](indexing.md)

### IndexedValue

**Definition**: Normalized value types stored in indexes for efficient lookup.

**Types**:
- `Int64(i64)`
- `Float64(OrderedFloat<f64>)` - Wrapper for comparable floats
- `Utf8(String)`
- `Boolean(bool)`
- `Timestamp(i64)`
- `Null`

**Purpose**: Provides consistent, comparable representation across Arrow types

**See Also**: [indexing.md](indexing.md)

## Views

### View

**Definition**: A named snapshot of container transformations stored within the bundle's manifest structure.

**Characteristics**:
- Named fork of a container capturing a transformation pipeline
- Stored in `view_{id}/_bundlebase/` subdirectory
- Has its own commit history
- **Read-only** when opened (returns `Bundle`, not `BundleBuilder`)
- **Dynamic inheritance** - automatically sees parent changes

**Storage Structure**:
```
container/
├── _bundlebase/
│   └── 00002def456.yaml     # Parent commit (contains CreateView op)
├── view_{uuid}/
│   └── _bundlebase/
│       ├── 00000000000000000.yaml  # View init: from="../"
│       └── 00001xyz789.yaml        # View operations
```

**Python API**:
- `create_view(name, builder)` - Create view from operations
- `view(name)` - Open view (returns `Bundle`)

**See Also**: [views.md](views.md)

### Named Fork

**Definition**: Alternative term for a view - emphasizes that it's a branching point with a name.

**See**: View (above)

### View Inheritance

**Definition**: Mechanism by which a view automatically includes parent container operations.

**How It Works**:
1. View's init commit contains `from: <parent_url>`
2. When opening, parent is recursively loaded first
3. View operations applied on top of parent operations
4. New parent commits automatically visible on next open

**See Also**: [views.md](views.md)

## Python Bindings

### PyBundle / PyBundleBuilder

**Definition**: Python wrapper classes that expose Rust container types via PyO3.

**Mapping**:
- `PyBundle` (Python) → `Bundlebase` (Rust read-only)
- `PyBundleBuilder` (Python) → `BundlebaseBuilder` (Rust mutable)

**Location**: `rust/bundlebase-python/src/`

**Characteristics**:
- Async methods for Python's async/await
- Error conversion (Rust errors → Python exceptions)
- Arrow FFI for zero-copy data transfer

**See Also**: [python-bindings.md](python-bindings.md)

### PySchema / PySchemaField

**Definition**: Python representation of Arrow schema for introspection.

**Usage**:
```python
schema = c.schema()
print(f"Columns: {len(schema.fields)}")
for field in schema.fields:
    print(f"  {field.name}: {field.data_type}")
```

**See Also**: [python-api.md](python-api.md)

### Streaming Execution

**Definition**: Memory-efficient execution pattern that processes data in batches rather than loading entire datasets.

**Rust Pattern**:
```rust
let stream = dataframe.execute_stream().await?;  // Constant memory
// vs
let batches = dataframe.collect().await?;  // Full materialization
```

**Python Pattern**:
```python
async for batch in stream_batches(container):
    process_batch(batch)  # Process incrementally
```

**Memory Impact**:
- **Streaming**: 10GB file = ~50MB peak RAM
- **Collection**: 10GB file = ~30GB peak RAM (3x size)

**Critical Rule**: ALWAYS use streaming, NEVER use `collect()`

**See Also**: [ai-rules.md](ai-rules.md), [anti-patterns.md](anti-patterns.md)

## Infrastructure

### ProgressScope

**Definition**: RAII (Resource Acquisition Is Initialization) pattern for tracking progress of long-running operations.

**Usage**:
```rust
let scope = tracker.scope("Loading data", total_rows);
for batch in batches {
    scope.update(batch.num_rows());
}
// Auto-completes when dropped
```

**Characteristics**:
- Automatically completes on drop
- Thread-safe progress updates
- Nested scopes supported

**See Also**: [progress.md](progress.md)

### ProgressTracker

**Definition**: Interface for tracking progress of operations.

**Implementations**:
- **DefaultTracker**: Console output with progress bars
- **NoOpTracker**: Silent mode for tests/scripts

**Global Access**: `get_tracker()`, `set_tracker()`, `with_tracker()`

**See Also**: [progress.md](progress.md)

### ProgressId

**Definition**: Unique identifier for a progress tracking scope.

**Type**: UUID-based identifier ensuring uniqueness across nested operations

**See Also**: [progress.md](progress.md)

## Technology Terms

### DataFusion

**Definition**: Apache Arrow-based SQL query engine used by Bundlebase for query execution.

**Version**: v51

**Usage**: Provides SQL parsing, query planning, optimization, and streaming execution

**Key Types**:
- `SessionContext` - Query execution context
- `DataFrame` - Lazy query representation
- `LogicalPlan` - Query plan before optimization
- `ExecutionPlan` - Physical query plan

**Website**: https://datafusion.apache.org/

### Arrow / Apache Arrow

**Definition**: Language-independent columnar memory format used for data representation.

**Version**: v57

**Key Concepts**:
- **RecordBatch**: Collection of columnar arrays (rows)
- **Schema**: Column names and types
- **Array**: Typed columnar data
- **SchemaRef**: `Arc<Schema>` for shared schema

**Benefits**:
- Zero-copy data sharing
- Efficient analytics
- Cross-language interoperability

**Website**: https://arrow.apache.org/

### PyO3

**Definition**: Rust bindings for Python - enables calling Rust from Python.

**Version**: v0.23

**Usage in Bundlebase**: Wraps Rust containers as Python classes with async methods

**Key Macros**:
- `#[pyclass]` - Define Python class in Rust
- `#[pymethods]` - Define Python methods
- `#[pyfunction]` - Define Python function

**Website**: https://pyo3.rs/

### Tokio

**Definition**: Asynchronous runtime for Rust.

**Version**: v1

**Usage**: Provides async/await execution, file I/O, and task scheduling

**See Also**: [ai-rules.md](ai-rules.md) - Runtime usage constraints

### Maturin

**Definition**: Build tool for Python packages with Rust extensions.

**Version**: 1.10.2+

**Usage**: `./scripts/maturin-dev.sh` - Build and install in development mode

**See Also**: [development.md](development.md)

## Status Values

### Container Status

**Values**:
- **Uncommitted**: Container has operations not yet committed
- **Committed**: All operations saved in manifest
- **Extended**: Container extends another container (has `from` field)

## Common Abbreviations

| Abbreviation | Full Term | Context |
|--------------|-----------|---------|
| **Arc** | Atomic Reference Counting | Rust shared ownership |
| **FFI** | Foreign Function Interface | Python-Rust boundary |
| **RAII** | Resource Acquisition Is Initialization | Progress scope pattern |
| **YAML** | YAML Ain't Markup Language | Manifest file format |
| **UUID** | Universally Unique Identifier | View IDs, progress IDs |
| **CSV** | Comma-Separated Values | Data format |
| **JSON** | JavaScript Object Notation | Data format |
| **SQL** | Structured Query Language | Query language |
| **E2E** | End-to-End | Test type |
| **PyPI** | Python Package Index | Distribution platform |
| **CLI** | Command-Line Interface | Terminal tool |

---

**Consistency Note**: When writing code or documentation, always use these exact terms. Don't invent synonyms (e.g., use "BundleBuilder" not "mutable container" or "builder container").
