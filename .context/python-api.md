# Python API Reference

This is the **async** API (`import bundlebase`). For the synchronous wrapper see [sync-api.md](sync-api.md). The classes are `PyBundleBuilder` (mutable, from `create()`/`extend()`) and `PyBundle` (read-only, from `open()`/`view()`).

## Basic Usage Example

```python
import bundlebase

# Create a writable bundle builder with a working directory
c = await bundlebase.create("memory:///test_bundle")  # Returns PyBundleBuilder

# Attach data source
c = await c.attach("userdata.parquet")

# Register a user-defined SQL scalar function (temporary, for Python runtime)
await c.import_temp_function(
    name="acme.double_val",
    from_="python::my_module:double_val"
)
# Use in SQL: SELECT acme.double_val(id) FROM bundle

# Or register a persistent function (non-Python runtimes only)
await c.import_function(
    name="acme.weather",
    from_="ipc::weather-binary"
)

# Transform data (modifies the bundle in place)
await c.drop_column("title")
await c.rename_column("first_name", "full_name")
await c.filter("SELECT * FROM bundle WHERE salary > $1", [50000])

# Access metadata
print(f"Name: {c.name}")            # property
print(f"Rows: {await c.num_rows()}")  # async method
print(f"Schema: {await c.schema()}")  # async method

# Query - returns a QueryResult (does NOT modify the bundle)
result = await c.query("SELECT id, full_name, salary FROM bundle")
df = await result.to_pandas()

# Or convert the whole bundle to a preferred format
df = await c.to_pandas()  # pandas DataFrame
# df = await c.to_polars()  # Polars DataFrame
# data = await c.to_dict()  # Dictionary of lists

# Commit the bundle state
await c.commit("Added data and transformations")
```

## Mutable Operations and Chaining

**Important:** All transformation methods mutate the bundle in place and resolve to the builder, so `c = await c.attach(...)` and `await c.attach(...)` are equivalent.

Mutation methods actually return an `OperationChain` (see `python/src/bundlebase/chain.py`), which queues operations and executes them all on a single `await`:

```python
c = await (bundlebase.create(path)        # CreateChain
          .attach("data.parquet")
          .drop_column("title")
          .rename_column("name", "full_name"))
```

Only methods registered in `_original_methods` in `python/src/bundlebase/__init__.py` are chainable — that dict must be kept in sync when new Rust binding methods are added. Read methods (`schema()`, `num_rows()`, `explain()`) and result-returning methods (`query()`, `fetch()`) are not chainable. Conversion methods (`to_pandas`, `to_polars`, `to_numpy`, `to_dict`, `as_pyarrow_stream`) can terminate a chain: they execute the queued operations first, then convert.

**Directory-Based Persistence:**
```python
# Bundle created with data_dir stores manifests in {data_dir}/_bundlebase/
c = await bundlebase.create("/my/bundle/dir")
await c.attach("data.parquet")
await c.commit("Initial data load")  # Creates versioned manifest

# Later, reopen the bundle (read-only PyBundle)
b = await bundlebase.open("/my/bundle/dir")

# To modify it, extend (returns an ExtendChain -> PyBundleBuilder)
c = await b.extend().attach("more_data.parquet")
```

## Supported Mutation Methods

- `attach(location, pack="base")` - Add data source (CSV columns import as text; use `cast_column` to convert types)
- `detach_block(location)` - Remove an attached block
- `replace_block(old_location, new_location)` - Repoint a block to a new location
- `create_source(connector, args, pack="base", fetch=True)` - Define a connector-backed source
- `fetch(pack="base", mode="add")` / `fetch_all(mode="add")` - Pull new/changed data from sources (return `FetchResults` lists, not the builder)
- `drop_column(name)` - Remove column
- `rename_column(old_name, new_name)` - Rename column
- `normalize_column_names()` - Convert all column names to lowercase+underscore identifiers
- `add_column(name, expression)` - Add a computed column using a SQL expression
- `cast_column(name, new_type)` - Cast column to a different data type
- `update(set_where)` - Update rows matching a condition (e.g., `"SET salary = 100 WHERE id = 1"`)
- `delete(where_clause)` - Delete rows matching a WHERE condition
- `always_delete(where_clause)` - Register persistent auto-delete rule (applies to all future attaches)
- `drop_always_delete(where_clause=None)` - Remove one or all always-delete rules
- `always_update(set_where)` - Register persistent auto-update rule (applies to all future attaches)
- `drop_always_update(rule_text=None)` - Remove one or all always-update rules
- `filter(query, params=None)` - Filter rows with a full SELECT query (e.g., `"SELECT * FROM bundle WHERE salary > $1"`)
- `join(name, expression, location=None, join_type=None)` - Join with another source (location optional; attach later with `attach(loc, pack=name)`)
- `drop_join(name)` / `rename_join(old, new)` - Manage joins
- `set_name(name)` - Set bundle name
- `set_description(description)` - Set bundle description
- `save_config(scope, key, value)` / `set_config(scope, key, value)` - Persist or session-set config values
- `import_connector(name, from_, platform="*/*", src=None)` - Define a persistent custom connector
- `import_temp_connector(name, from_, platform="*/*")` - Define a session-only custom connector (required for Python runtime)
- `drop_connector(name, platform=None)` / `rename_connector(old, new)` - Manage connectors
- `drop_temp_connector(name, platform=None)` - Drop runtime-only connector
- `import_function(name, from_, platform="*/*")` - Define a persistent SQL function (types auto-detected from manifest)
- `import_temp_function(name, from_, platform="*/*")` - Define a session-only SQL function (types auto-detected from manifest)
- `drop_function(name, platform=None, input_types=None)` / `rename_function(old, new)` - Manage functions
- `create_view(name, sql)` - Create a named view from a SQL query
- `rename_view(old, new)` / `drop_view(name)` - Manage views
- `create_index(columns, index_type, args=None, name=None)` - Create a "btree" or "text" index
- `drop_index(column)` / `rebuild_index(column)` / `reindex()` - Index maintenance
- `create_report(id, name, description, content)` - Create or replace a stored report template
- `drop_report(id)` - Drop a stored report
- `generate_report(id, output, no_branding=False)` - Generate a PDF from a stored report
- `reset()` - Drop all uncommitted operations
- `undo()` - Undo the last uncommitted operation
- `commit(message)` - Persist uncommitted changes
- `export_tar(tar_path, gzip=False)` - Export the bundle as a single tar archive
- `export_empty(path)` - Export bundle structure only (recipients `fetch()` their own data)

## Querying

`query()` executes SQL and returns a `QueryResult` wrapper (it does **not** modify the bundle):

```python
result = await c.query("SELECT * FROM bundle WHERE id > $1", [100], hard_limit=1000)

df = await result.to_pandas()      # or to_polars() / to_dict()
# or stream:
async for batch in result.stream_batches():
    process(batch)
```

A `QueryResult` wraps a one-shot stream — after any conversion or `stream_batches()` consumes it, run the query again for fresh results. `describe_data(columns)`, `test_connector(name, **kwargs)`, and `test_temp_connector(from_, **kwargs)` also return `QueryResult`.

## Schema Introspection

`schema()` is an async method returning a `PySchema`:

```python
c = await bundlebase.create("memory:///test_bundle")
await c.attach("users.parquet")

schema = await c.schema()

# Schema properties
print(f"Column count: {len(schema)}")
print(f"Is empty: {schema.is_empty()}")

# Iterate over all fields
for field in schema.fields:
    print(f"  {field.name}: {field.data_type} (nullable={field.nullable})")

# Get specific field by name
id_field = schema.field("id")
print(f"ID type: {id_field.data_type}")
print(f"ID nullable: {id_field.nullable}")

# String representation
print(schema)  # Human-readable schema dump
```

**PySchema methods and properties:**
- `fields` (property) - List of all PySchemaField objects
- `field(name)` - Get specific field by name, raises ValueError if not found
- `__len__()` / `__getitem__(idx)` - Number of fields / field by index
- `is_empty()` - Check if schema has any fields
- `__str__()` - Pretty-printed schema with all columns and types

**PySchemaField properties:**
- `name` - Column name (string)
- `data_type` - Apache Arrow data type (e.g., Int32, Utf8View, Float64)
- `nullable` - Whether column can contain NULL values (boolean)

## Data Conversion Methods

After building your data transformation pipeline, convert results to your preferred format:

```python
c = await bundlebase.create("memory:///test_bundle")
await c.attach("data.parquet")

# Option 1: Pandas DataFrame (widely used in data science)
df_pandas = await c.to_pandas()
print(df_pandas.describe())

# Option 2: Polars DataFrame (fast, modern alternative)
df_polars = await c.to_polars()
print(df_polars.schema)

# Option 3: NumPy arrays (for scientific computing)
arrays = await c.to_numpy()
ids = arrays["id"]  # numpy.ndarray

# Option 4: Python dictionaries (generic Python usage)
data = await c.to_dict()
names = data["name"]  # list of values

# Option 5: Raw PyArrow batches (low-level access; materializes everything)
batches = await c.as_pyarrow()
for batch in batches:
    print(f"Batch with {batch.num_rows} rows")
```

**When to use each method:**

| Format | Use Case | Memory |
|--------|----------|--------|
| **to_pandas()** | Data analysis, visualization | Full materialization (~2x dataset at peak) |
| **to_polars()** | Modern alternative to pandas | Full materialization (~2x dataset at peak) |
| **to_numpy()** | NumPy array operations, ML | Full materialization (via `as_pyarrow()`) |
| **to_dict()** | Generic Python usage, JSON serialization | Full materialization |
| **as_pyarrow()** | Legacy batch access (uses DataFusion `collect()`) | Full materialization |
| **stream_batches()** | **Incremental processing of large datasets** | **Bounded** (one batch at a time) |

**Important notes:**
- All conversion methods are `async` - must be awaited
- Raise `ValueError` if the bundle has no data attached
- Return types:
  - `to_pandas()` → `pandas.DataFrame`
  - `to_polars()` → `polars.DataFrame`
  - `to_numpy()` → `Dict[str, numpy.ndarray]`
  - `to_dict()` → `Dict[str, list]`
  - `as_pyarrow()` → `List[pyarrow.RecordBatch]` (legacy, prefer `stream_batches()`)
  - `stream_batches()` → `AsyncIterator[pyarrow.RecordBatch]`

## Streaming API for Large Datasets

What actually streams with bounded memory:

- `bundlebase.stream_batches(c)` / `QueryResult.stream_batches()` - pull one batch at a time from a Rust `execute_stream()`; only the current batch (plus whatever you keep) is in memory
- `as_pyarrow_stream()` - the underlying `PyRecordBatchStream` object

What does **not**:

- `to_pandas()` / `to_polars()` stream batches from Rust but **accumulate every batch** and then build the final DataFrame (`pd.concat` / `pa.Table.from_batches`) - peak memory is roughly 2x the dataset size (see `python/src/bundlebase/conversion.py`)
- `as_pyarrow()` uses DataFusion `collect()` and materializes the entire result

For datasets that don't fit comfortably in RAM, use `stream_batches()`:

```python
import bundlebase

c = await bundlebase.create()
c = await c.attach("huge_file.parquet")  # 10GB+ file

# Process one batch at a time - bounded memory usage
total_rows = 0
async for batch in bundlebase.stream_batches(c):
    # Each batch is a pyarrow.RecordBatch
    chunk_df = batch.to_pandas()

    # Process chunk (memory freed after iteration)
    process_chunk(chunk_df)
    total_rows += batch.num_rows

print(f"Processed {total_rows} rows with minimal memory")
```

**Use cases for streaming:**
- **Custom aggregations**: Incremental statistics, rolling calculations
- **Data validation**: Check each batch independently, early termination
- **Streaming writes**: Process and write batches to another system
- **Memory constraints**: Datasets larger than (or near) available RAM

### Direct Stream Access

For advanced use cases, access the underlying stream object:

```python
# Get the stream object directly
stream = await c.as_pyarrow_stream()

# Read batches one at a time
while True:
    batch = await stream.next_batch()
    if batch is None:  # Stream exhausted
        break

    # Process batch
    print(f"Batch: {batch.num_rows} rows")
```

### Streaming Performance Guidelines

**DO:**
- ✅ Use `to_pandas()` / `to_polars()` when the result comfortably fits in RAM (remember the ~2x peak)
- ✅ Use `stream_batches()` for large datasets or custom incremental processing
- ✅ Process batches independently - avoid accumulating results

**DON'T:**
- ❌ Don't use `as_pyarrow()` or `to_pandas()` for datasets near or beyond RAM size
- ❌ Don't collect all batches into a list - defeats streaming purpose
- ❌ Don't implement custom batching - use built-in streaming

**Example: Bad pattern (defeats streaming)**
```python
# BAD: Collecting all batches defeats streaming
batches = []
async for batch in stream_batches(c):
    batches.append(batch)  # Accumulates in memory!
table = pa.Table.from_batches(batches)  # Full materialization
```

**Example: Good pattern (maintains streaming)**
```python
# GOOD: Process and discard each batch
results = []
async for batch in stream_batches(c):
    # Compute aggregate per batch
    chunk_sum = batch.column('amount').to_pandas().sum()
    results.append(chunk_sum)
    # batch is garbage collected here

total = sum(results)  # Only aggregates kept in memory
```

### Streaming Architecture Details

**Rust Layer:**
- `PyRecordBatchStream` wraps DataFusion's `SendableRecordBatchStream`
- `execute_stream()` provides lazy batch iteration (not `collect()`)
- Each batch freed when Python reference count drops to zero

**Python Layer:**
- `stream_batches()` async generator yields batches one at a time (bounded memory)
- `to_pandas()` / `to_polars()` consume that stream but accumulate all chunks before building the final DataFrame (~2x dataset peak)

## Parameterized SQL and Filtering

Use parameterized queries to safely handle user input. Note that `filter()` takes a **full SELECT query**, not a bare predicate:

```python
import bundlebase

c = await bundlebase.create("memory:///test_bundle")
await c.attach("users.parquet")

# Basic filter with parameters ($1 placeholder)
await c.filter("SELECT * FROM bundle WHERE salary > $1", [50000.0])

# Multiple parameters
await c.filter(
    "SELECT * FROM bundle WHERE salary > $1 AND department = $2",
    [50000.0, "Engineering"],
)

# Parameterized SQL query
results = await c.query(
    "SELECT name, salary FROM bundle WHERE salary > $1 AND active = $2",
    [50000.0, True]
)
```

**Parameter syntax:**
- **Placeholders**: Use `$1`, `$2`, `$3`, etc. for positional parameters
- **List matching**: Parameters list must match placeholders in order
- **Type coercion**: DataFusion automatically converts Python types to Arrow types

**Security and best practices:**
- Always use parameterized queries for user input (prevents SQL injection)
- Parameters are type-checked before execution
- Supported types: numbers, strings, booleans, None (NULL)

**Examples with different data types:**

```python
# String parameter
await c.filter("SELECT * FROM bundle WHERE country = $1", ["USA"])

# Date/timestamp parameter
await c.filter("SELECT * FROM bundle WHERE created_at > $1", ["2023-01-01"])

# Boolean parameter
await c.filter("SELECT * FROM bundle WHERE is_active = $1", [True])

# Multiple types
await c.filter(
    "SELECT * FROM bundle WHERE name = $1 AND age > $2 AND active = $3",
    ["Alice", 25, True]
)
```

## Custom Connectors

### import_connector()

Defines a persistent custom connector. The connector definition and entrypoint are stored in the bundle's commit history.

```python
# IPC connector (subprocess)
await c.import_connector('acme.weather', 'ipc::my_connector')

# Shared library connector
await c.import_connector('acme.weather', 'ffi::libweather.so')

# Platform-specific
await c.import_connector('acme.weather', 'ffi::libweather.so', platform='linux/amd64')
```

**Parameters:**
- `name` - Dot-separated connector name (e.g., `"acme.weather"`)
- `from_` - URI in `runtime::entrypoint` format (e.g., `"ipc::my_connector"`); supported runtimes: `"ipc"`, `"ffi"`, `"java"`, `"docker"` (not `"python"` — use `import_temp_connector` instead)
- `platform` (optional) - Target platform (e.g., `"linux/amd64"`, default `"*/*"`)
- `src` (optional, keyword-only) - Path to a source archive (zip) bundled alongside the connector

### import_temp_connector()

Defines a session-only connector. Nothing is persisted — the connector exists only at runtime. Required for Python in-process connectors. Works on both `PyBundle` (read-only) and `PyBundleBuilder`.

```python
# Python in-process connector (most common)
await c.import_temp_connector('acme.weather', 'python::my_module:MyConnector')

# Temporary IPC connector
await c.import_temp_connector('acme.weather', 'ipc::my_connector')
```

**Parameters:** Same as `import_connector()` (minus `src`), but the `from_` parameter accepts all runtimes including `"python"`.

### drop_connector()

Removes a connector, or removes a connector for a specific platform only.

```python
# Drop entire connector
await c.drop_connector('acme.weather')

# Drop connector for a specific platform only
await c.drop_connector('acme.weather', platform='linux/amd64')
```

### drop_temp_connector()

Removes runtime-only connector entries.

```python
# Drop all temporary entries for the connector
await c.drop_temp_connector('acme.weather')

# Drop for a specific platform
await c.drop_temp_connector('acme.weather', platform='*/*')
```

**Returns:** A message describing what was dropped.

See [Custom Connectors](../docs/guide/custom-connectors/index.md) for full details on runtimes, SDKs, and protocol.

## Views

Views are named SQL snapshots that are stored and versioned within the bundle. They automatically inherit new commits from their parent bundle.

### Creating Views

`create_view(name, sql)` takes a **SQL string** and returns the new view's builder:

```python
c = await bundlebase.create("/path/to/bundle")
await c.attach("customers.csv")
await c.commit("Initial data")

# Create view from a SQL query
await c.create_view("adults", "SELECT * FROM bundle WHERE age > 21")
await c.commit("Add adults view")
```

**Method signature:**
```python
await c.create_view(name: str, sql: str) -> PyBundleBuilder
```

**Parameters:**
- `name` - Unique name for the view
- `sql` - SQL query defining the view contents (e.g., `"SELECT * FROM bundle WHERE age > 21"`)

**Returns:** A `PyBundleBuilder` for the created view

### Opening and Listing Views

Use `view()` to open a previously created view as a read-only `PyBundle`, and `views()` to list them:

```python
# Open view by name or ID - returns PyBundle (read-only)
v = await c.view("adults")

# List views: {view_id: view_name}
print(c.views())

# Access view operations
print(f"View has {len(v.operations())} operations")
```

Views are not queryable as SQL tables — `view()` returns a sub-bundle scoped to the view's SQL, and the usual `query("... FROM bundle ...")` idioms apply against the returned bundle.

**Raises:** `ValueError` if the view doesn't exist.

### View Characteristics

**Read-only:** Views return `PyBundle` not `PyBundleBuilder`, so transformation methods are not available:
```python
v = await c.view("adults")
# v.filter(...)  # ERROR: PyBundle doesn't have filter()
# v.attach(...)  # ERROR: PyBundle doesn't have attach()
```

**Dynamic inheritance:** Views automatically see new commits from their parent bundle:
```python
# Create view
await c.attach("data-1.csv")
await c.create_view("active", "SELECT * FROM bundle WHERE status = 'active'")
await c.commit("v1")

# Add more data to parent
b = await bundlebase.open("bundle")
c2 = await b.extend().attach("data-2.csv")
await c2.commit("v2")

# View automatically includes new parent data
v = await c2.view("active")  # Sees both data-1.csv and data-2.csv
```

### Management

- `rename_view(old_name, new_name)` - Rename a view
- `drop_view(name)` - Remove a view

See [views.md](views.md) for detailed documentation on views architecture, storage structure, and advanced usage.

## Native Semantic Metadata

Both opened bundles and builders expose `await semantic_model() -> dict`, an
Ossie snapshot of committed plus pending metadata, and
`await export_semantic_model(path)`. The snapshot is detached; editing it has no
effect until explicitly applied. Schema validation uses the pinned offline
Ossie schema. Strict snapshots/exports accept the supported ANSI SQL subset;
unsupported DataFusion or imported-function expressions remain valid native
metrics and surface an actionable portability error at this boundary.

Builders provide:

- `await apply_semantic_model(document: dict, replace=False)` — one undoable
  batch of ordinary operations; omitted properties remain unchanged by default.
- `await preview_semantic_model(document: dict, replace=False)` — the same diff
  as a list of `PyOperation` values, without recording it.
- `await import_semantic_model(path, replace=False)` — parse JSON/YAML and apply
  the same diff; extension must be `.json`, `.yaml`, or `.yml`.
- `await set_column_label(name, label=None)`.
- `await set_metric_expression(name, expression)`.
- `await set_metric_description(name, description=None)`.
- `await set_ai_context(target, context=None, *, name=None, member=None)`.

AI targets are `model`, `dataset`, `column`, and `metric`; column and metric
require `name`. `member` selects one top-level object member. `None` explicitly
clears a context or member. Known object members have pinned schema types:
`instructions` is a string; `synonyms` and `examples` are string arrays.
Extension members may use any schema-permitted JSON value. Whole string
contexts are atomic. A member set after a string or absent context establishes
an object; a member removal in that situation does nothing.

```python
model = await builder.semantic_model()
model["datasets"][0]["description"] = "One row per order"
operations = await builder.preview_semantic_model(model)
await builder.apply_semantic_model(model)
await builder.set_metric_expression("revenue", "SUM(amount - discount)")
await builder.set_ai_context("column", {"synonyms": ["net total"]}, name="amount")
await builder.execute("DECLARE PRIMARY KEY (order_id)")
await builder.export_semantic_model("sales.yaml")
```

`execute(sql, params=None)` runs normal Bundlebase commands and returns
`QueryResult`; use it for `ALTER`, `DECLARE`, `SHOW SEMANTIC MODEL`, and other
native commands. The command dispatcher enforces read-only bundle restrictions.
The sync API exposes the same methods without `await`.

`replace=True` selects all supported editable metadata in the opened model:
omitted properties and key declarations are cleared and omitted metrics are
dropped; physical columns are unchanged. Preview the full diff before applying
replacement. Document application is atomic, produces no empty change for an
identical snapshot, preserves existing pending work on validation failure, and
never commits implicitly. A single undo removes the entire applied batch.

Convenience readers `metrics()` and `column_descriptions()` consume
`BundleFacade::semantic_state()`, as do metric computation, Arrow descriptions,
and MCP. Metric expression updates do not overwrite independent descriptions
or AI context. Stable column references survive renames. Keys declare intent
and do not enforce data constraints.

See [Native Semantic Metadata](architecture.md#native-semantic-metadata) and
[ADR-012](decisions/012-native-semantic-model.md).

## Other Useful Methods

Brief pointers to the rest of the surface (see `python/src/bundlebase/__init__.pyi` for full signatures):

- `c.status()` - Uncommitted changes (`PyBundleStatus`)
- `c.history()` - List of `PyCommit` objects
- `c.operations()` - List of `PyOperation` objects
- `c.version` / `c.url` / `c.id` / `c.name` / `c.description` - Properties
- `await c.explain(verbose=False, analyze=False, format=None, sql=None)` - Query plan stream
- `await c.describe_data(columns)` - Per-column quality stats (`QueryResult`)
- `await c.describe_function(name)` - Function metadata (`QueryResult`)
- `await c.verify_data()` - Verify file integrity via SHA256 hashes
- `bundlebase.test_datafile(name)` / `bundlebase.random_memory_url()` - Test helpers
