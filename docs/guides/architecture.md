# Bundlebase Architecture

## Three-Tier Architecture

### Bundlebase Trait
Common interface for all container types (`rust/bundlebase/src/bundle.rs`):
- Defines methods for schema introspection, querying, and data conversion
- Implemented by both Bundlebase and BundlebaseBuilder

### Bundlebase
Read-only container loaded from disk:
- Represents a committed snapshot of the container
- Loaded from versioned manifests in `{data_dir}/_manifest/`
- Cannot be modified directly
- Can be extended via `extend(new_data_dir)` to create a new BundlebaseBuilder
- Immutable and thread-safe

### BundlebaseBuilder
Mutable container for modifications:
- Wraps an Bundlebase with a working directory
- Tracks new operations applied since the base container
- All modification methods mutate in-place and return `&mut self`
- Methods: `attach()`, `remove_column()`, `rename_column()`, `filter()`, `select()`, `join()`, `define_function()`, `set_name()`, `set_description()`
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
- **DefineFunction**: Register custom data generation functions
- **SetName**: Set container name
- **SetDescription**: Set container description
- **IndexData**: Track row indexing metadata

## Adapter System

Plugin architecture for data sources (`src/data_adapter/`):
- **CsvPlugin**: CSV file support
- **JsonPlugin**: Line-delimited JSON support
- **ParquetPlugin**: Apache Parquet support
- **FunctionPlugin**: Custom function data sources via `function://` URLs

## Function System

Custom data generation framework (`src/functions/`) with complete lifecycle:

**Core Components:**
- **FunctionSig**: Function signature (name + output Arrow schema)
- **FunctionImpl**: Trait for creating data generators
- **FunctionRegistry**: Shared registry (via `Arc<RwLock>`) for all function definitions
- **DataGenerator**: Trait for paginated data generation (called with page 0, 1, 2... until returns None)
- **StaticImpl**: Built-in implementation for static RecordBatch data
- **PythonFunctionImpl**: Bridge to Python functions (converts Python callables to Rust DataGenerators)

**Function Lifecycle:**

1. **Define signature**: `container.define_function(FunctionSig::new("my_func", schema))`
   - Registers function name and output schema in shared FunctionRegistry
   - Creates DefineFunction operation (stored but doesn't affect DataFrame until attached)

2. **Set implementation**: `container.set_impl("my_func", Arc::new(impl))`
   - Stores the actual implementation in FunctionRegistry
   - Implementation must match the registered signature
   - Directly modifies shared registry (no operation created)

3. **Attach function**: `container.attach("function://my_func")`
   - FunctionPlugin looks up signature in registry
   - Creates FunctionDataAdapter linking to the function
   - AttachBlock operation stores the adapter

4. **Query execution**:
   - AttachBlock reads data via the adapter
   - Adapter retrieves implementation from registry
   - Implementation creates a DataGenerator
   - Generator's `next(page)` method called repeatedly: `next(0)`, `next(1)`, `next(2)`...
   - Continues until generator returns `None`
   - Each page returns a RecordBatch of data

## Clone Semantics and Arc Usage

Both container types use `Arc` (Atomic Reference Counting) for shared state:

**Bundlebase:**
- **Cheap cloning**: BundlebaseState is shared via Arc
- **Shared FunctionRegistry**: All clones access the same global function registry
- **Immutable snapshots**: Each clone represents the same committed state

**BundlebaseBuilder:**
- **Cheap cloning**: Arc-based state and directory reference
- **Independent operation tracking**: Each clone can have different new operations
- **Shared base**: All clones reference the same base (committed) container
- **Shared data directory**: All clones write to the same directory when committed
- **Enables branching**: Can create multiple modified versions from one base

**Key implications:**
- Cloning containers is fast (just Arc counter increments)
- All containers share the global FunctionRegistry (intentional for function reuse)
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
