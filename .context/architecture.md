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
- Methods: `attach()`, `remove_column()`, `rename_column()`, `standardize_column_names()`, `filter()`, `select()`, `join()`, `create_function()`, `create_connector()`, `set_name()`, `set_description()`
- Can be committed via `commit(message)` to create a new versioned snapshot
- Can be re-opened via `open_extending(url)` to load the latest state

### BundlebaseState
Shared state extracted for both container types:
- Schema (Arrow SchemaRef)
- Metadata (name, description)
- Row count tracking
- SessionContext for DataFusion
- Function entries (user-defined SQL functions)
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
- **CreateFunction**: Register a user-defined SQL scalar function
- **DropFunction**: Remove a user-defined SQL function
- **SetName**: Set container name
- **SetDescription**: Set container description
- **IndexData**: Track row indexing metadata

## Adapter System

Plugin architecture for data sources (`src/data_adapter/`):
- **CsvPlugin**: CSV file support
- **JsonPlugin**: Line-delimited JSON support
- **ParquetPlugin**: Apache Parquet support
- **BundlebasePlugin**: References other committed bundles via `bundle://` (filesystem) or `bundle+<scheme>://` (remote) URLs

## User-Defined SQL Functions

Custom SQL function system supporting both scalar and aggregate functions.

**Key Files:**
- `src/bundle/function_definition.rs` — `FunctionEntry`, `FunctionKind`, `FunctionRegistry`
- `src/function/scalar.rs` — DataFusion `ScalarUDFImpl` bridge
- `src/function/aggregate.rs` — DataFusion `AggregateUDFImpl` bridge with `PythonAccumulator` and `LibAccumulator`
- `src/function/python_bridge.rs` — Trait for Python function invocation
- `src/function/lib_bridge.rs` — FFI layer for native shared library (.so/.dylib) functions

**Core Components:**
- **FunctionEntry**: Stores function metadata (name, input/return types, runner, logic, platform, kind)
- **FunctionKind**: `Scalar` (row → row) or `Aggregate` (many rows → one result per group)
- **ScalarFunction**: DataFusion `ScalarUDFImpl` bridge for scalar functions
- **AggregateFunction**: DataFusion `AggregateUDFImpl` bridge for aggregate functions
- **PythonAccumulator**: `Accumulator` impl that delegates to Python class methods
- **LibAccumulator**: `Accumulator` impl that delegates to C ABI aggregate symbols via FFI
- **Runner**: Execution environment — `python`, `lib`, `java`, `docker`, `ipc` (shared with connectors)
- **Platform**: OS/arch pattern for multi-platform support (e.g., `linux/amd64`, `*/*`)

**Lib Runner (FFI Layer):**
- `lib_bridge::parse_lib_logic()` — Parses `path:symbol` convention (colon separates library path from symbol name)
- `lib_bridge::load_library()` — Loads shared libraries with a global `Mutex<HashMap>` cache
- `lib_bridge::invoke_lib_scalar()` — Converts Arrow arrays to FFI, calls C function, converts back
- `lib_bridge::LibAccumulator` — Wraps opaque `void*` state, calls `_create_state/_accumulate/_evaluate/_free_state` symbols
- `lib_bridge::load_lib_manifest()` — Calls `bundlebase_functions()` C symbol for bulk discovery
- `lib_bridge::load_ipc_manifest()` — Runs `exec --bundlebase-functions` for IPC discovery
- `CREATE FUNCTIONS FROM` command uses manifests to register multiple functions at once

**Function Lifecycle:**

1. **Create function**: `CREATE FUNCTION acme.double_val(Int64) RETURNS Int64 WITH (runner = 'ipc', logic = './my_func')`
   - Validates dotted name (exactly one dot, alphanumeric parts)
   - Validates Arrow type names for inputs and return type
   - Optional `type = 'aggregate'` in WITH clause (default: `scalar`)
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
   - `CREATE TEMPORARY FUNCTION` — session-only, not persisted, allows Python runner
   - `CREATE FUNCTION` — persisted as operation, rejects Python runner (can't be bundled)
   - Temporary overrides persistent at resolution time

**Python Aggregate Interface:**
- Class with four methods: `create_state()`, `accumulate(state, values)`, `merge(state1, state2)`, `evaluate(state)`
- State is a PyArrow scalar (simple types: Int64, Float64, Utf8, etc.)
- Each accumulator gets its own class instance (stateful per partition)
- `logic` format: `module:ClassName` (same as scalar `module:function`)

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

## Design Patterns

1. **Plugin Architecture**: Extensible adapter system for new data sources
2. **Operation Pipeline**: Declarative transformation chain
3. **Lazy Evaluation**: Deferred execution until query time
4. **Trait-Based Polymorphism**: Clean interfaces via Rust traits
5. **Cross-Language Integration**: Seamless Rust-Python interop via PyO3 and Arrow
6. **Arc-Based Sharing**: Efficient cloning with shared state
7. **Three-Tier Container Architecture**: Flexible immutability and versioning
8. **Manifest-Based Persistence**: Version history with 'from' chain
