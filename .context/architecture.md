# Bundlebase Architecture

## Three-Tier Architecture

### BundleFacade Trait
Common interface for both bundle types (`rust/bundlebase/src/bundle/facade.rs`):
- Defines methods for schema introspection (`schema()`, `bundle_schema()`), querying (`query()`, `dataframe()`, `num_rows()`), metadata (`name()`, `version()`, `history()`, `operations()`), views, reports, and export
- Implemented by both `Bundle` and `BundleBuilder`
- Catalog schema providers and the CLI work against `dyn BundleFacade` so they support either type

### Bundle
A bundle loaded from persistent storage (`rust/bundlebase/src/bundle.rs`):
- Represents committed state, loaded from versioned manifests in `{data_dir}/_bundlebase/`
- **Interior mutability**: all mutable state lives in `Arc<RwLock<T>>` fields (~25 of them: operations, packs, sources, views, reports, config, cached dataframe, etc.)
- Mutation happens only through `pub(crate) async fn apply_operation(&self, op)` — external code cannot record operations on a bare `Bundle`
- `Clone` shares all `Arc<RwLock<...>>` state: mutations in one clone are visible in all clones (intentional — schema providers see live state)
- Thread-safe; shareable as `Arc<Bundle>` across async tasks, server sessions, and Python
- Can be extended via `extend(data_dir)` to create a `BundleBuilder`

### BundleBuilder
Working copy with change tracking (`rust/bundlebase/src/bundle/builder.rs`):
- Wraps an `Arc<Bundle>` plus: `in_progress_change`, uncommitted `status` (list of `BundleChange`s), and pending deletes/updates accumulated by DELETE/UPDATE commands
- Modification methods take `&self` and return `Result<&Self, BundlebaseError>` for chaining with `?` (interior mutability — see ADR-005)
- Core methods on the struct: `create()`, `extend()`, `commit(message)`, `reset()`, `undo()`, `do_change()`, `run_command()`, `apply_operation()`
- Convenience methods (`attach()`, `drop_column()`, `rename_column()`, `cast_column()`, `filter()`, `join()`, `import_function()`, `import_connector()`, `create_view()`, `set_name()`, `set_description()`, `create_index()`, ...) live on the **`BundleBuilderExt` trait** in `rust/bundlebase-command/src/builder_ext.rs`; each runs a command through `run_command` for change tracking and rollback-on-failure
- `commit(message)` persists all accumulated changes as a new versioned manifest

There is no separate `BundleState` type — schema/name tracking is handled by `BundleSchema` (a `ColumnId -> user-visible name` map, `rust/bundlebase/src/bundle/bundle_schema.rs`), and everything else lives directly on `Bundle`.

### Workspace Crates

The Rust workspace has 13 crates under `rust/`:
`bundlebase` (core), `bundlebase-ossie` (portable semantic document types and schema validation), `bundlebase-command` (command parsing + `BundleBuilderExt`), `bundlebase-cli` (CLI binary), `bundlebase-python` (PyO3 bindings), `bundlebase-common` (errors, RowId, ObjectId, Platform, arrow type parsing), `bundlebase-data` (reader plugins, DataReaderFactory), `bundlebase-io` (storage abstraction), `bundlebase-catalog` (DataFusion `bundle_info.*` schema providers), `bundlebase-connector` (connector definitions), `bundlebase-index` (BTree indexes), `bundlebase-udf` (function runtimes/bridges), `bundlebase-report` (report generation).

## Operation Pipeline

Operations are recorded in `Bundle::operations` and applied in sequence when the DataFrame is (re)built. The full set is defined in one place — the `define_any_operation!` macro invocation in `rust/bundlebase/src/bundle/operation.rs`, which generates the `AnyOperation` enum, its `Operation` impl, and `From` impls for each concrete op type.

Representative operations (see `operation.rs` for the complete list of ~39):
- **AttachBlock / DetachBlock / ReplaceBlock**: Add, remove, or replace data blocks (attach unions rows)
- **AddColumn / DropColumn / RenameColumn / CastColumn / DropCastColumn**: Column manipulation
- **Filter**: Apply a stored SELECT against the current DataFrame
- **CreateJoin / DropJoin / RenameJoin**: Join with other data sources
- **Delete / UpdateData / AlwaysDelete / AlwaysUpdate (+ Drop variants)**: Row-level changes and persistent rules
- **CreateSource / ImportConnector / RenameConnector / DropConnector**: Data source and connector management
- **ImportFunction / RenameFunction / DropFunction**: User-defined SQL functions
- **CreateView / RenameView / DropView**: Named views
- **CreateIndex / DropIndex / IndexBlocks**: Row indexing
- **CreateReport / DropReport**: Stored markdown report templates
- **SetName / SetDescription / SetMinVersion / SetMaxVersion / SaveConfig / UpdateVersion**: Metadata

**Adding a new operation**: create `bundle/operation/your_op.rs`, declare the module and re-export at the top of `operation.rs`, and add one line to the `define_any_operation!` invocation. See [prompts/add-operation.md](prompts/add-operation.md).

## Reader Plugin System

Plugin architecture for data sources. Format readers implement the `ReaderPlugin` trait in `rust/bundlebase-data/src/plugin.rs`; the bundle-reference plugin lives in `rust/bundlebase/src/data/plugin/`:
- **CsvPlugin / TsvPlugin**: CSV/TSV file support. All columns import as Utf8 text — type inference from samples is unreliable. The reader reads only the header row for schema inference.
- **JsonlPlugin**: Line-delimited JSON support (`.jsonl`). Types are preserved from the JSON data.
- **ParquetPlugin**: Apache Parquet support (`.parquet`). Types are preserved from the embedded schema.
- **BundlebasePlugin**: References other committed bundles via `bundle://` (filesystem) or `bundle+<scheme>://` (remote) URLs (`rust/bundlebase/src/data/plugin/bundlebase_reader.rs`)

Plugins are registered with `DataReaderFactory` (`rust/bundlebase-data/src/reader_factory.rs`). A plugin's `reader()` method decides whether it applies (by extension and/or `read_options`) and returns `Option<Arc<dyn DataReader>>`.

## User-Defined SQL Functions

Custom SQL function system supporting both scalar and aggregate functions.

**Key Files** (function system lives in the `bundlebase-udf` and `bundlebase-common` crates; `rust/bundlebase/src/lib.rs` re-exports it as the `function` module):
- `rust/bundlebase-udf/src/function_entry.rs` — `FunctionEntry`, `FunctionKind`, `FunctionRegistry`
- `rust/bundlebase-common/src/arrow_types.rs` — Arrow type parsing (`parse_arrow_type_name`)
- `rust/bundlebase-common/src/platform.rs` — `Platform` struct for Docker-style os/arch matching
- `rust/bundlebase-udf/src/bridge/scalar.rs` — DataFusion `ScalarUDFImpl` bridge
- `rust/bundlebase-udf/src/bridge/aggregate.rs` — DataFusion `AggregateUDFImpl` bridge with `PythonAccumulator` and `LibAccumulator`
- `rust/bundlebase-udf/src/bridge/python_bridge.rs` — Trait for Python function invocation
- `rust/bundlebase-udf/src/bridge/ffi_bridge.rs` — FFI layer for native shared library (.so/.dylib) functions
- `rust/bundlebase-udf/src/bridge/manifest.rs` — Shared `Manifest`/`ManifestEntry` types used by all runtimes
- `rust/bundlebase-udf/src/runtime/` — Per-runtime loaders (`python.rs`, `ffi.rs`, `java.rs`, `docker.rs`, `ipc.rs`)

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
   - `IMPORT FUNCTION` — persisted as operation, rejects module-backed Python (file-backed `python::./x.py:f` is bundled)
   - Temporary overrides persistent at resolution time

4. **Bundled entrypoint files**: `copy_into_bundle()` stores the file content-addressed in the data dir
   and records the relative hash path (`ab/cdef0123456789.udf.bin`). `UdfRuntime::resolve_path()`
   maps it to a local path: the data dir itself for `file://` bundles, otherwise a per-user cache
   (`{cache_dir}/bundlebase/udf/{xxh3(data_dir url)}/{rel}`). `resolve_local()` (async) also
   downloads into that cache and verifies the content hash. Call `resolve_local()` at an async
   point before anything spawns or `dlopen`s the file; `ImportFunctionOp::apply` does it before
   the synchronous `FunctionRegistry` registration.

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

Both bundle types use `Arc` (Atomic Reference Counting) for shared state:

**Bundle:**
- **Cheap cloning**: every mutable field is `Arc<RwLock<T>>`; `Clone` just bumps Arc counters
- **Shared state**: clones share state with the original — mutations in one clone are visible in all clones. This is intentional: schema providers (`BundleInfoSchemaProvider`) hold a clone and must see live state. (The cached DataFrame holder gets a fresh `RwLock` per clone.)

**BundleBuilder:**
- **Cheap cloning**: shares the underlying `Arc<Bundle>`
- **Independent change tracking**: `in_progress_change`, `status`, and pending deletes/updates are deep-copied per clone

**Key implications:**
- Cloning bundles is fast (just Arc counter increments)
- `commit()` persists uncommitted changes as a new manifest version
- `Bundle::open(url).extend(...)` loads the latest committed state into a new BundleBuilder

## Operation Pattern: check/apply/apply_dataframe

Operations implement the `Operation` trait (`rust/bundlebase/src/bundle/operation.rs`):

```rust
pub trait Operation: Send + Sync + Clone + Serialize + Debug + Into<AnyOperation> {
    fn describe(&self) -> String;

    /// Validate against the current bundle (immediate, fail fast)
    async fn check(&self, bundle: &Bundle) -> Result<(), BundlebaseError>;

    /// Apply side effects to the bundle via interior mutability (immediate) —
    /// e.g. register a pack/view/index definition. No-op for many ops.
    async fn apply(&self, bundle: &Bundle) -> Result<(), DataFusionError>;

    /// Transform the DataFrame (LAZY — runs when the dataframe is rebuilt).
    /// Default implementation returns df unchanged.
    async fn apply_dataframe(
        &self,
        df: DataFrame,
        ctx: Arc<SessionContext>,
        bundle_schema: &mut BundleSchema,
    ) -> Result<DataFrame, BundlebaseError>;

    // Also: version() (content hash), allowed_on_view(), to_empty()
}
```

**Application flow:**

1. **When recording an operation** (`Bundle::apply_operation`):
   - `check()` validates (immediate)
   - `apply()` updates bundle state via locks (immediate)
   - Op is pushed onto `Bundle::operations`; the operation's `version()` hash is fed into the bundle version; the cached DataFrame and `BundleSchema` are cleared

2. **When querying** (`Bundle::dataframe()`):
   - If a cached DataFrame exists, return it
   - Otherwise rebuild: start from the base pack table, compute `resolve_cast_ops()` (cancels reverted casts), run each active op's `apply_dataframe()` in order (`CreateJoin` ops execute the join inline), then rename `col_<id>` columns to user-visible names and cache the result

**Why this pattern?**
- Validation happens early (fail fast)
- Actual data transformation is deferred until needed
- Operations serialize to/from manifests (serde via `AnyOperation`)
- Each operation contributes a content hash to the bundle version

**Registration:** the `define_any_operation!` macro in `operation.rs` generates the `AnyOperation` enum, its trait impl, and `From<ConcreteOp>` impls. New operations are registered by adding one line to the macro invocation.

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

**Location:** `rust/bundlebase-python/src/record_batch_stream.rs`

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

**Location:** `python/src/bundlebase/conversion.py`

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
c = await bundlebase.create("memory://test_bundle")
await c.attach("users1.parquet")  # 1000 rows
await c.attach("users2.parquet")  # 500 rows
results = await c.to_dict()       # 1500 total rows
```

## Column Identity and Internal Naming

Each column has a stable `ColumnId` (64-bit hex, e.g., `0a3f1b2c9d4e7f80`) assigned at attach time. Internally, all column names use the format `col_<column_id>` throughout the pipeline:

- **DataBlock** (`data_block.rs`): The `TableProvider::schema()` returns `col_<id>` field names. Reader batches are renamed from physical names to `col_<id>` at scan time (zero-copy schema swap).
- **Operations**: `apply_dataframe()` methods work with `col_<id>` column names in the DataFrame. `RenameColumnOp` is metadata-only (just updates the name map). `DropColumnOp`/`CastColumnOp` reference `col_<id>` directly.
- **SQL in operations**: All stored SQL (WHERE clauses, expressions, filters) uses `col_<id>` references. The command layer translates user-visible names to `col_<id>` at creation time via `BundleBuilder::translate_sql()` / `BundleSchema::translate_sql()`.
- **Final rename**: `dataframe()` in `bundle.rs` applies all operations, then as the last step calls `BundleSchema::rename_to_real_names()` to rename every `col_<id>` column to its user-visible name.

This design means stored operations are resilient to column renames — an `ALWAYS DELETE WHERE col_<id> < 0` remains valid even if the column is renamed.

**Key files**: `bundle/bundle_schema.rs` (helpers: `generate_internal_name()`, `parse_internal_name()`, `translate_sql()`, `rename_to_real_names()`), `bundle/data_block.rs`, `bundle/schema_rename_filter.rs`.

## Native Semantic Metadata

`BundleFacade::semantic_state()` folds the ordinary operation history into one
`SemanticState`. It includes committed and pending operations when called on a
builder. `metrics()`, column description accessors, Arrow descriptions, and the
Ossie view consume this state; none maintains a separate mutable document store.
Model name and description retain the existing bundle properties.

The state owns model and dataset AI context, dataset description, per-column
description/label/AI context, named metric expression/description/AI context,
one ordered primary key, and separate unique-key declarations. Columns use
stable `ColumnId` values; metrics use exact case-sensitive names. Stored metric
SQL references internal column/function IDs and is rendered with current names
at public read boundaries. Keys declare intent and never scan or constrain data.
Unique-key identity normalizes member sets while preserving declared display
order. Primary-key order is significant.

Property edits use concrete operations registered in `define_any_operation!`:
`SetMetricExpressionOp`, `SetMetricDescriptionOp`, `SetColumnLabelOp`,
`ClearColumnDescriptionOp`, `SetAiContextOp`, `SetAiContextMemberOp`,
`SetDatasetDescriptionOp`, `DeclareKeyOp`, and `DropKeyOp`, alongside the existing
metric creation/removal and column description operations. Model description
clearing uses `ClearDescriptionOp`. Their metadata `apply()` methods generally
do nothing and retain the default unchanged `apply_dataframe()` behavior.

The resolver folds operations in manifest order. Later writes to the same
property win; writes to different properties survive independently. Dropping a
metric removes its properties, setters against an absent metric do nothing,
and a later explicit creation starts with fresh metadata. A definition updating
an existing metric changes only its expression. Descriptions use separate
property operations. Metadata for dropped columns and keys with missing columns is
omitted. An invalidated metric expression remains readable until computation
or export attempts to plan it.

An AI context is a string or an object. Whole replacements overlap all member
updates. Setting a member after a string or absent context establishes an
object; removing a member from a string or absent context does nothing. Arrays
are atomic member values. JSON null is a value only when the pinned member's
schema permits it; removal is explicit. Known members are `instructions`
(string), `synonyms` (string array), and `examples` (string array); extension
members follow the schema's permissive additional-property rule.

Authored operations validate targets and metric SQL before recording. Manifest
loading checks persisted semantic operation structure without repeating target
existence or SQL planning against a concurrently changed schema. Other
operations retain their normal validation. This keeps ordinary combined
metadata histories replayable without masking malformed metadata values.

`BundleFacade::semantic_model()` projects a valid Ossie document on demand.
`bundlebase-ossie` vendors an immutable upstream schema and examples, validates
JSON/YAML offline, and is independent of Arrow and DataFusion. The adapter in
`bundle/semantic_model.rs` accepts one dataset for the opened bundle, rejects
unsupported dimensions/relationships and physical schema changes, and lowers
a supplied document into ordinary property operations. MERGE leaves omitted
metadata unchanged. Explicit REPLACE clears omitted supported metadata and
removes omitted metrics within the opened model. Object AI context updates
lower to top-level member writes; omission never becomes a persisted wildcard.
An identical document produces no change. Application groups the diff in one
`do_change`, so validation failure preserves pending work and undo removes the
whole batch. Reads and exports do not commit or record operations.

**Key files:** `bundle/semantic_state.rs`, `bundle/semantic_model.rs`,
`bundle/operation/semantic_validation.rs`, `rust/bundlebase-ossie/`.

**Decision:** [ADR-012](decisions/012-native-semantic-model.md).

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
