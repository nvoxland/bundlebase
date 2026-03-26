# Bundlebase Architecture

## Three-Tier Architecture

### Bundlebase Trait
Common interface for all container types (`rust/bundlebase/src/bundle.rs`):
- Defines methods for schema introspection, querying, and data conversion
- Implemented by both Bundlebase and BundlebaseBuilder

### Bundlebase
Read-only container loaded from disk:
- Represents a committed snapshot of the container
- Loaded from versioned manifests in `{data_dir}/_bundlebase/`
- Cannot be modified directly
- Can be extended via `extend(new_data_dir)` to create a new BundlebaseBuilder
- Immutable and thread-safe

### BundlebaseBuilder
Mutable container for modifications:
- Wraps an Bundlebase with a working directory
- Tracks new operations applied since the base container
- All modification methods mutate in-place and return `&mut self`
- Methods: `attach()`, `drop_column()`, `rename_column()`, `standardize_column_names()`, `filter()`, `select()`, `join()`, `import_function()`, `import_connector()`, `set_name()`, `set_description()`
- Can be committed via `commit(message)` to create a new versioned snapshot
- Can be re-opened via `open_extending(url)` to load the latest state

### BundlebaseState
Shared state extracted for both container types:
- Schema (Arrow SchemaRef)
- Metadata (name, description)
- Row count tracking
- SessionContext for DataFusion
- Function registry
- Adapter factory

## Operation Pipeline

Operations are recorded and applied in sequence when querying:
- **AttachBlock**: Add data from sources (union operation)
- **RemoveColumns**: Filter out columns
- **RenameColumn**: Rename columns
- **Filter**: Filter rows based on predicates
- **Select**: Select specific columns
- **Join**: Join with other data sources
- **Query**: Execute custom SQL
- **ImportFunction**: Register a user-defined SQL scalar function
- **RenameFunction**: Rename a function definition
- **DropFunction**: Remove a user-defined SQL function
- **ImportConnector**: Register a connector definition
- **RenameConnector**: Rename a connector definition
- **DropConnector**: Remove a connector definition
- **SetName**: Set container name
- **SetDescription**: Set container description
- **IndexData**: Track row indexing metadata
- **DescribeConnector**: Returns metadata table (name, runtime, entrypoint, platform, temporary) for a registered connector

## Adapter System

Plugin architecture for data sources (`src/data_adapter/`):
- **CsvPlugin**: CSV file support (`.csv`). All columns import as Utf8 text — type inference from samples is unreliable.
- **JsonPlugin**: Line-delimited JSON support (`.json`, `.jsonl`). Types are preserved from the JSON data.
- **ParquetPlugin**: Apache Parquet support (`.parquet`). Types are preserved from the embedded schema.
- **BundlebasePlugin**: References other committed bundles via `bundle://` (filesystem) or `bundle+<scheme>://` (remote) URLs

Each plugin implements `FileFormatConfig` (extensions, format, file source) and `DataReader` (schema, data source, statistics). The CSV reader reads only the header row for schema inference (no type inference), so all columns are naturally Utf8.

## User-Defined SQL Functions

Custom SQL function system supporting both scalar and aggregate functions.

**Key Files:**
- `src/bundle/function_entry.rs` — `FunctionEntry`, `FunctionKind`, `FunctionRegistry`
- `src/arrow_types.rs` — Arrow type parsing (`parse_arrow_type_name`)
- `src/platform.rs` — `Platform` struct for Docker-style os/arch matching
- `src/function/scalar.rs` — DataFusion `ScalarUDFImpl` bridge
- `src/function/aggregate.rs` — DataFusion `AggregateUDFImpl` bridge with `PythonAccumulator` and `LibAccumulator`
- `src/function/python_bridge.rs` — Trait for Python function invocation
- `src/function/ffi_bridge.rs` — FFI layer for native shared library (.so/.dylib) functions
- `src/function/manifest.rs` — Shared `Manifest`/`ManifestEntry` types used by all runtimes

**Core Components:**
- **FunctionEntry**: Stores function metadata (name, input/return types, runtime, entrypoint, platform, kind)
- **FunctionKind**: `Scalar` (row → row), `Aggregate` (many rows → one result per group), or `TableValued` (returns a table; registration only, execution not yet implemented)
- **ScalarFunction**: DataFusion `ScalarUDFImpl` bridge for scalar functions
- **AggregateFunction**: DataFusion `AggregateUDFImpl` bridge for aggregate functions
- **PythonAccumulator**: `Accumulator` impl that delegates to Python class methods
- **LibAccumulator**: `Accumulator` impl that delegates to C ABI aggregate symbols via FFI
- **IpcAccumulator**: `Accumulator` impl that delegates to IPC subprocess via JSON-RPC + Arrow IPC; state is opaque (only state IDs cross the wire)
- **Runtime**: Execution environment — `python`, `lib`, `java`, `docker`, `ipc` (shared with connectors)
- **Platform**: OS/arch pattern for multi-platform support (e.g., `linux/amd64`, `*/*`)

**FFI Runtime (FFI Layer):**
- `ffi_bridge::parse_lib_entrypoint()` — Parses `path:symbol` convention (colon separates library path from symbol name)
- `ffi_bridge::load_library()` — Loads shared libraries with a global `Mutex<HashMap>` cache
- `ffi_bridge::invoke_lib_scalar()` — Converts Arrow arrays to FFI, calls C function, converts back
- `ffi_bridge::LibAccumulator` — Wraps opaque `void*` state, calls `_create_state/_accumulate/_evaluate/_free_state` symbols
- `ffi_bridge::load_lib_manifest()` — Calls `bundlebase_functions()` C symbol for bulk discovery
- IPC/Java manifest loaders live in their respective runtime files (`ipc.rs`, `java.rs`)
- `ipc_bridge` module — JSON-RPC + Arrow IPC protocol for scalar invoke and aggregate (`create_state`/`accumulate`/`merge`/`evaluate`)
- `IMPORT FUNCTION namespace.* FROM 'runtime::entrypoint'` command uses manifests to register multiple functions at once

**IPC Health Check:**
- All SDKs respond to a `ping` method with `"pong"`, used by Bundlebase to verify subprocess liveness.

**Complex Arrow Types:**
- The type system supports `List<T>`, `Struct<name:type,...>`, `Map<K,V>`, `Decimal128(precision,scale)`, `LargeUtf8`, and `LargeBinary` in addition to primitive types.

**FETCH DRY RUN:**
- `FETCH` and `FETCH ALL` commands support an optional `DRY RUN` modifier that previews changes without executing them.

**IPC Subprocess Cache:**
- Each `Bundle` owns a `SubprocessCache` (`Arc<Mutex<HashMap<String, ...>>>`)
- IPC subprocesses are cached **per-Bundle**, not globally — each connection/session gets its own processes
- Subprocesses are spawned on first use and live as long as the Bundle (killed on Drop)
- Cache key is the entrypoint string (e.g., `python:my_functions.py`)

**Function Lifecycle:**

1. **Load function**: `IMPORT FUNCTION acme.double_val FROM 'ipc::./my_func'`
   - Validates dotted name (exactly one dot, alphanumeric parts)
   - Input types, return type, and function kind (scalar/aggregate) are auto-detected from the runtime
   - Creates a `FunctionEntry` stored in the bundle's `function_entries` list
   - Registers with DataFusion via `register_function_with_datafusion()`:
     - Scalar → `register_udf(ScalarUDF)`
     - Aggregate → `register_udaf(AggregateUDF)`

2. **Use in SQL**:
   - Scalar: `SELECT acme.double_val(id) FROM bundle`
   - Aggregate: `SELECT acme.my_sum(amount) FROM bundle GROUP BY category`
   - Window: `SELECT acme.my_sum(amount) OVER (ORDER BY id) FROM bundle`
   - DataFusion automatically supports any aggregate UDF with `OVER()` clauses

3. **Temporary vs persistent**:
   - `IMPORT TEMP FUNCTION` — session-only, not persisted, allows Python runtime
   - `IMPORT FUNCTION` — persisted as operation, rejects Python runtime (can't be bundled)
   - Temporary overrides persistent at resolution time

**Python Aggregate Interface:**
- Class with four methods: `create_state()`, `accumulate(state, values)`, `merge(state1, state2)`, `evaluate(state)`
- State is a PyArrow scalar (simple types: Int64, Float64, Utf8, etc.)
- Each accumulator gets its own class instance (stateful per partition)
- `entrypoint` format: `module:ClassName` (same as scalar `module:function`)

**Naming Convention:**
- Single-level dotted namespace: `namespace.function_name` (e.g., `acme.double_val`)
- Same validation shared with connectors via `parse_dotted_name()`
- Maps to Arrow Flight schemas for database client intellisense

## Clone Semantics and Arc Usage

Both container types use `Arc` (Atomic Reference Counting) for shared state:

**Bundlebase:**
- **Cheap cloning**: BundlebaseState is shared via Arc
- **Immutable snapshots**: Each clone represents the same committed state

**BundlebaseBuilder:**
- **Cheap cloning**: Arc-based state and directory reference
- **Independent operation tracking**: Each clone can have different new operations
- **Shared base**: All clones reference the same base (committed) container
- **Shared data directory**: All clones write to the same directory when committed
- **Enables branching**: Can create multiple modified versions from one base

**Key implications:**
- Cloning containers is fast (just Arc counter increments)
- `commit()` creates a new Bundlebase snapshot
- `open_extending(url)` loads the latest BundlebaseBuilder from manifests

## Three-Phase Operation Pattern

Operations implement a three-phase pattern via the `Operation` trait:

1. **Validation phase**: `check()` - checks if operation is valid
2. **State modification phase**: `reconfigure()` - updates schema, row count, metadata
3. **DataFrame transformation phase**: `apply_dataframe()` - called LAZILY when query() is executed

**Application flow:**

1. **When adding operation**:
   - Validate operation (immediate)
   - Update schema, row count (immediate)
   - Store for later execution

2. **When querying**:
   - Transform DataFrame through each operation (lazy)

**Why this pattern?**
- Schema tracking is immediate (know column names/types without executing query)
- Validation happens early (fail fast)
- Actual data transformation is deferred until needed
- Row count tracking is accurate
- Supports operation serialization via `config()`

## Streaming Execution Architecture

Bundlebase uses **streaming execution** throughout to handle datasets larger than available RAM. This is a core architectural decision that affects all query execution paths.

### Streaming vs Collection

**Collection (OLD, avoided):**
```rust
// BAD: Materializes entire dataset in memory
let batches = dataframe.collect().await?;  // Vec<RecordBatch>
// Memory usage: 3x dataset size
```

**Streaming (CURRENT, default):**
```rust
// GOOD: Processes batches one at a time
let stream = dataframe.execute_stream().await?;  // SendableRecordBatchStream
// Memory usage: Constant per batch (~8-64MB)
```

### Rust Layer: PyRecordBatchStream

**Location:** `rust/Bundlebase/src/python/record_batch_stream.rs`

**Purpose:** Exposes DataFusion's streaming execution to Python with zero-copy data transfer.

**Key components:**
- Wraps `SendableRecordBatchStream` (from DataFusion)
- Uses `tokio::sync::Mutex` for async-safe stream access
- Implements `next_batch()` for Python iteration
- Schema cached at stream creation for O(1) access

**Memory characteristics:**
- Single batch in memory at a time
- Python GC frees batches as they're processed
- No batch accumulation in Rust layer

### Python Layer: stream_batches()

**Location:** `python/src/Bundlebase/conversion.py`

**Purpose:** Async generator for batch-by-batch processing in Python.

**Implementation:**
```python
async def stream_batches(container) -> AsyncIterator[pa.RecordBatch]:
    stream = await container.as_pyarrow_stream()  # Get Rust stream
    while True:
        batch = await stream.next_batch()         # Fetch one batch
        if batch is None:
            break
        yield batch                                # Yield to caller
        # batch garbage collected here if caller doesn't hold reference
```

**Conversion function integration:**
- `to_pandas()`: streams batches → list of DataFrames → `pd.concat()`
- `to_polars()`: streams batches → Arrow Table → Polars DataFrame
- Both maintain constant memory per batch, NOT proportional to dataset size

### Performance Characteristics

**Memory usage comparison (10GB Parquet file):**

| Method | Peak Memory | Scalability |
|--------|-------------|-------------|
| `collect()` + `to_pandas()` | ~30GB (3x) | OOM on large files |
| `execute_stream()` + streaming | ~50MB | Constant, file-size independent |

**Batch sizes:**
- Default: DataFusion chooses optimal size (typically 8K-64K rows)
- Depends on schema complexity and data types
- Automatically balanced for throughput vs memory

### Critical Implementation Rules

**For Rust developers:**
1. ✅ Always use `execute_stream()` for query execution, never `collect()`
2. ✅ Pass `SendableRecordBatchStream` to Python via `PyRecordBatchStream`
3. ✅ Let DataFusion manage batch sizes - don't override without benchmarking
4. ❌ Never accumulate batches in `Vec<RecordBatch>` before returning to Python

**For Python developers:**
1. ✅ Use `to_pandas()` / `to_polars()` for most cases - they stream internally
2. ✅ Use `stream_batches()` for custom incremental processing
3. ✅ Process batches independently - avoid accumulating in lists
4. ❌ Don't call `as_pyarrow()` for large datasets - it materializes everything
5. ❌ Don't collect all batches before processing - defeats streaming purpose

### When Streaming Is NOT Used

**Legacy methods** (kept for compatibility, but discouraged for large datasets):
- `as_pyarrow()` - returns `List[RecordBatch]` (full materialization)
- `to_numpy()` - requires full dataset in Arrow format
- `to_dict()` - requires full dataset in Arrow format

**Recommendation:** For large datasets, use `stream_batches()` with custom processing instead of these methods.

## UNION Behavior

Multiple `attach()` calls perform SQL UNION ALL (combining rows, not joining):

**Schema alignment:**
- If sources have different schemas, missing columns are filled with NULLs
- Column order is preserved from the first source
- Later sources must have compatible types for overlapping columns

**Example:**
```python
c = await Bundlebase.create("memory:///test_container")
await c.attach("users1.parquet")  # 1000 rows
await c.attach("users2.parquet")  # 500 rows
results = await c.to_dict()       # 1500 total rows
```

## Schema Tracking

Schema is stored as `Arc<LinkedHashMap<String, String>>`:
- **LinkedHashMap**: Preserves column insertion order
- **Arc**: Cheap cloning when creating new containers
- **String → String**: Maps column name → Arrow type string (e.g., "Int32", "Utf8View")

Schema updates are immediate - `container.schema()` work instantly without executing the query.

## CLI Modes

The CLI (`rust/bundlebase-cli/`) provides subcommands for interacting with bundles. All modes share the same `BundleFacade` trait, command parsing, and query execution infrastructure. Subcommand dispatch is in `main.rs`; each subcommand is implemented in its own module under `cmd/` (one module per subcommand).

### `bundlebase repl` (Interactive REPL)

Interactive command-line interface using `reedline`. Supports SQL commands and `/meta-commands` with tab completion and history.

**Key files:** `cmd/repl_cmd.rs`, `repl.rs`, `repl/commands.rs`, `repl/commands/sql.rs`

### `bundlebase query` (Read-Only Query)

Non-interactive read-only mode: execute one or more semicolon-separated SQL queries and exit. Opens bundle via `Bundle::open()`. SQL can be passed as a positional argument or piped via stdin. All statements are validated before any execute.

**Key files:** `cmd/query_cmd.rs`

### `bundlebase extend` (Mutating Command)

Non-interactive read-write mode: execute one or more semicolon-separated mutating commands and exit. Opens bundle via `Bundle::open().extend()`. All statements are validated before any execute. Auto-commits after all commands complete if there are uncommitted changes. `bundlebase execute` is a hidden alias.

**Key files:** `cmd/extend_cmd.rs`

### `bundlebase server` (Flight SQL Server)

Arrow Flight SQL server over gRPC. Each authenticated client gets its own session with independent bundle state. Supports JDBC/ODBC clients like DBeaver.

**Key files:** `cmd/server_cmd.rs`, `flight.rs`, `flight/server.rs`, `flight/service.rs`

### `bundlebase mcp` (MCP Server)

Model Context Protocol server over stdio for AI assistant integration. Supports multiple bundles open simultaneously, each identified by a unique key. Bundles can be pre-opened via `--bundle` flag or opened/closed dynamically via tools. Exposes tools (`open_bundle`, `create_bundle`, `close_bundle`, `list_bundles`, `query`, `schema`, `count`, `sample`, `status`, `history`) that AI assistants call with a bundle key. Uses the `rmcp` crate.

**Key files:** `mcp.rs`, `mcp/server.rs`, `mcp/tools.rs`

**Shared infrastructure reused by MCP:**
- `repl/commands.rs` — command parsing (`parse()`, `execute()`)
- `repl/json_formatter.rs` — Arrow RecordBatch to JSON conversion
- `repl/commands/sql.rs` — SQL execution with 1000-row hard limit

## Identifiers and Case Sensitivity

Bundlebase is always case-sensitive. Column names, join aliases, view names, and all other identifiers preserve their exact case. `Revenue`, `revenue`, and `REVENUE` are three different columns.

This is intentional: bundlebase works with disparate data sources (CSVs, APIs, Parquet files, databases) that each have their own casing conventions. Normalizing case would silently break data from sources that rely on specific casing. DataFusion is configured with `enable_ident_normalization = false` (set in `bundle.rs` and all ephemeral `SessionContext` instances).

**Quoted identifiers:** The bundlebase grammar accepts double-quoted identifiers for names containing spaces, dots, or special characters (e.g., `RENAME COLUMN "Result/Value" TO result_value`). Quotes are purely syntactic — they don't affect case behavior.

**Key files:** `grammar.pest` (identifier rule), `pest_parser.rs` (`extract_identifier`, `quote_identifier`), `bundle.rs` (ident normalization config)

## Design Patterns

1. **Plugin Architecture**: Extensible adapter system for new data sources
2. **Operation Pipeline**: Declarative transformation chain
3. **Lazy Evaluation**: Deferred execution until query time
4. **Trait-Based Polymorphism**: Clean interfaces via Rust traits
5. **Cross-Language Integration**: Seamless Rust-Python interop via PyO3 and Arrow
6. **Arc-Based Sharing**: Efficient cloning with shared state
7. **Three-Tier Container Architecture**: Flexible immutability and versioning
8. **Manifest-Based Persistence**: Version history with 'from' chain
