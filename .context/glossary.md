# Bundlebase Glossary

This document defines project-specific terminology used throughout the Bundlebase codebase and documentation. Use these exact terms for consistency.

## Core Concepts

### BundleFacade (Trait)

**Definition**: The common read/query interface shared by both bundle types.

**Usage**: Defines methods for schema introspection (`schema()`, `bundle_schema()`), querying (`query()`, `dataframe()`, `num_rows()`), metadata, views, reports, and export. Catalog schema providers and the CLI work against `dyn BundleFacade`.

**Implemented By**: `Bundle` (committed) and `BundleBuilder` (working copy)

**Location**: `rust/bundlebase/src/bundle/facade.rs`

**See Also**: [architecture.md](architecture.md)

### Bundle (Committed Bundle)

**Definition**: A bundle loaded from persistent storage, representing committed state.

**Characteristics**:
- Loaded from versioned manifests in `{data_dir}/_bundlebase/`
- Uses interior mutability: all mutable state lives in `Arc<RwLock<T>>` fields, so it is thread-safe and `Arc`-shareable
- State changes only happen through the `pub(crate)` `apply_operation()` path — external code cannot record operations on a bare `Bundle`
- `Clone` shares Arc state: mutations in one clone are visible in all clones
- Can be extended via `extend(new_data_dir)` to create a `BundleBuilder`

**Python Type**: `PyBundle` (read/query wrapper)

**Usage Example**:
```python
# Load committed bundle
c = await bundlebase.open("/path/to/bundle")
df = await c.to_pandas()  # Can read, cannot record operations
```

**See Also**: [architecture.md](architecture.md)

### BundleBuilder (Working Copy)

**Definition**: A working copy of a bundle for building and transforming data, with uncommitted-change tracking.

**Characteristics**:
- Wraps an `Arc<Bundle>` plus change tracking (`in_progress_change`, uncommitted `status`, pending deletes/updates)
- Modification methods take `&self` and return `Result<&Self, BundlebaseError>` (interior mutability) — chainable with `?`
- Convenience methods (`attach()`, `filter()`, ...) live on the `BundleBuilderExt` trait in `rust/bundlebase-command/src/builder_ext.rs`
- Can be committed via `commit(message)` to create a versioned snapshot; `reset()`/`undo()` discard or roll back uncommitted changes

**Methods**: `attach()`, `drop_column()`, `rename_column()`, `cast_column()`, `filter()`, `join()`, `import_function()`, `create_view()`, `commit()`, ...

**Python Type**: `PyBundleBuilder` (mutation wrapper)

**Usage Example**:
```python
# Create working copy
c = await bundlebase.create("/path/to/bundle") \
    .attach("data.parquet") \
    .filter("age >= 18")  # Chainable mutations
await c.commit("Filtered adults")
```

**See Also**: [architecture.md](architecture.md), [python-api.md](python-api.md)

### BundleSchema

**Definition**: The schema-tracking structure threaded through the operation pipeline.

**Contains**:
- `ColumnId → user-visible name` map (including computed columns)
- The physical schema and per-column block locations
- Helpers: `translate_sql()` (user names → `col_<id>`), `rename_to_real_names()` (final rename), `internal_name()`, `column_id()`

**Usage**: Each operation's `apply_dataframe()` receives `&mut BundleSchema`; metadata-only operations (e.g. `RenameColumnOp`) just update the map.

**Location**: `rust/bundlebase/src/bundle/bundle_schema.rs`

**See Also**: [architecture.md](architecture.md)

## Operations

### Operation

**Definition**: A recorded transformation step in the data pipeline. The full set (~39 ops) is declared in the `define_any_operation!` macro in `rust/bundlebase/src/bundle/operation.rs`.

**Representative Types**:
- **AttachBlock / DetachBlock / ReplaceBlock**: Add, remove, or replace data blocks (attach unions rows)
- **AddColumn / DropColumn / RenameColumn / CastColumn**: Column manipulation
- **Filter**: Apply a stored SELECT against the current DataFrame
- **CreateJoin**: Join with other data sources
- **Delete / UpdateData / AlwaysDelete / AlwaysUpdate**: Row-level changes and persistent rules
- **ImportFunction / ImportConnector / CreateSource**: Functions, connectors, sources
- **CreateView / CreateIndex / CreateReport**: Views, indexes, reports
- **SetName / SetDescription**: Bundle metadata

**Lifecycle**:
1. **Validated** immediately via `check(&Bundle)` (fail fast)
2. **Bundle side effects** applied immediately via `apply(&Bundle)` (interior mutability; no-op for many ops)
3. **Recorded** onto `Bundle::operations`; the cached DataFrame is invalidated
4. **Executed on DataFrame** lazily during rebuild via `apply_dataframe(df, ctx, &mut BundleSchema)`

**See Also**: [architecture.md](architecture.md)

### Lazy Evaluation

**Definition**: Pattern where operations are recorded when called but only executed during query time.

**Example**:
```python
c = c.filter("age >= 18")    # RECORDED only
c = c.query("SELECT name FROM bundle")  # RECORDED only
df = await c.to_pandas()     # NOW both operations EXECUTE
```

**Benefit**: Enables query optimization and composable transformations

**See Also**: [ai-rules.md](ai-rules.md)

### Reader Plugin

**Definition**: Plugin component (`ReaderPlugin` trait) that handles loading data from specific source formats. Sometimes called an "adapter" in older docs.

**Built-in Plugins**:
- **CsvPlugin / TsvPlugin**: CSV/TSV file support (all columns import as Utf8)
- **JsonlPlugin**: Line-delimited JSON support
- **ParquetPlugin**: Apache Parquet support
- **BundlebasePlugin**: References other committed bundles via `bundle://` URLs

**Location**: `rust/bundlebase-data/src/plugin/` (BundlebasePlugin: `rust/bundlebase/src/data/plugin/`)

**See Also**: [architecture.md](architecture.md)

### Function Entries

**Definition**: The list of function definitions stored in each bundle's state.

**Characteristics**:
- Per-bundle storage of user-defined SQL functions
- Each entry contains metadata (name, input/return types, runtime, entrypoint, platform, kind)
- Registered with DataFusion's SessionContext for SQL execution
- Supports both persistent and temporary (session-scoped) entries

**See Also**: [architecture.md](architecture.md)

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

### BTree Index

**Definition**: Data structure that accelerates queries with equality, IN, and range predicates.

**Storage**: Content-addressed binary files with `.index.btree.rowmap` extension.

**Structure**:
```rust
pub struct BTreeIndex {
    column_name: String,
    data_type: DataType,
    blocks: Vec<IndexBlock>,       // Value -> RowId mappings
    directory: BlockDirectory,     // Block-level min/max
    total_entries: u64,
    total_rows: u64,
}
```

**Python API**:
- `create_index(column_name)` - Create index on column
- `rebuild_index(column_name)` - Rebuild existing index

**See Also**: [indexing.md](indexing.md)

### RowId

**Definition**: Unique identifier for a row, packing a compact block reference and a row number into a single `u64`.

**Structure**:
```rust
// rust/bundlebase-common/src/row_id.rs
pub struct RowId(u64); // RowId::new(block_ref: ObjectIdAlias, row_number: u32)
```

**Usage**: Internal tracking for row identification across blocks (indexes, tombstones, update overlays)

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

## Stored Reports

### Report / Stored Report

**Definition**: A markdown template with embedded `bundlebase` fenced code blocks, stored inside a bundle and identified by a unique id.

**Characteristics**:
- Id: alphanumeric, underscores, hyphens (e.g., `monthly-sales`)
- Contains markdown with `bundlebase` fenced blocks for charts/tables
- Content-addressed via SHA-256 hash
- Stored as operations (CreateReportOp/DropReportOp) in the commit chain
- Queryable via `bundle_info.reports` table

**SQL**: `CREATE REPORT`, `DROP REPORT`, `SHOW REPORTS`, `GENERATE REPORT`

**Python**: `create_report()`, `drop_report()`, `generate_report()`

**See Also**: [reports guide](../docs/guide/reports.md)

## Python Bindings

### PyBundle / PyBundleBuilder

**Definition**: Python wrapper classes that expose Rust container types via PyO3.

**Mapping**:
- `PyBundle` (Python) → `Bundle` (Rust, committed)
- `PyBundleBuilder` (Python) → `BundleBuilder` (Rust, working copy)

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

**Version**: v53 (see workspace `Cargo.toml` for current)

**Usage**: Provides SQL parsing, query planning, optimization, and streaming execution

**Key Types**:
- `SessionContext` - Query execution context
- `DataFrame` - Lazy query representation
- `LogicalPlan` - Query plan before optimization
- `ExecutionPlan` - Physical query plan

**Website**: https://datafusion.apache.org/

### Arrow / Apache Arrow

**Definition**: Language-independent columnar memory format used for data representation.

**Version**: v58 (see workspace `Cargo.toml` for current)

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

**Version**: v0.28 (see `rust/bundlebase-python/Cargo.toml` for current)

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
