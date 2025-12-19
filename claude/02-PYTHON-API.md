# Python API Reference

## Basic Usage Example

```python
import Bundlebase
import pyarrow as pa

# Create a writable BundlebaseBuilder with a working directory
c = await Bundlebase.create("memory:///test_container")  # Returns BundlebaseBuilder

# Attach data source
await c.attach("userdata.parquet")

# Define custom function
def my_data(page: int, schema: pa.Schema) -> pa.RecordBatch | None:
    if page == 0:
        return pa.record_batch(
            data={
                "id": [1, 2, 3],
                "name": ["Alice", "Bob", "Charlie"]
            },
            schema=schema
        )
    return None

await c.define_function(
    name="my_data",
    output={"id": "Int64", "name": "Utf8"},  # output not schema
    func=my_data  # func not function
)
await c.attach("function://my_data")

# Transform data (modifies container in place)
await c.remove_column("title")
await c.rename_column("first_name", "full_name")

# Chain multiple operations (each mutates the container)
await c.filter("salary > $1", [50000])
await c.select("id", "full_name", "salary")

# Access metadata (properties, not methods)
print(f"Name: {c.name}")  # Property access
print(f"Rows: {c.num_rows}")
print(f"Schema: {c.schema}")

# Query results - convert to preferred format
df = await c.to_pandas()  # pandas DataFrame
# df = await c.to_polars()  # Polars DataFrame
# data = await c.to_dict()  # Dictionary of lists

# Commit the container state
await c.commit("Added data and transformations")
```

## Mutable Operations Pattern

**Important:** All transformation methods mutate the container in place:

```python
c = await Bundlebase.create("/path/to/container")
c = await c.attach("data.parquet")

# c is now modified with the attached data
c = await c.remove_column("title")  # c is further modified
c = await c.rename_column("name", "full_name")  # c continues to be modified

# All modifications are on the same container instance
```

**Directory-Based Persistence:**
```python
# Container created with data_dir stores manifests in {data_dir}/_manifest/
c = await Bundlebase.create("/my/container/dir")
await c.attach("data.parquet")
await c.commit("Initial data load")  # Creates versioned manifest

# Later, reopen the same container
c = await Bundlebase.open("/my/container/dir")
```

**Note:** FunctionRegistry is shared across all container instances. When you define a function in one container, it's available globally. This is intentional for function reuse.

## Supported Mutation Methods

- `attach(url)` - Add data source
- `remove_column(name)` - Remove column
- `rename_column(old_name, new_name)` - Rename column
- `filter(where_clause, params)` - Filter rows
- `select(*columns)` - Select columns
- `join(url, expression, join_type)` - Join with another source
- `query(sql, params)` - Execute custom SQL
- `set_name(name)` - Set container name
- `set_description(description)` - Set container description
- `define_function(name, output, func)` - Define custom function

## Schema Introspection

BundlebaseBuilder provides rich schema information through the `PySchema` and `PySchemaField` classes:

```python
c = await Bundlebase.create("memory:///test_container")
await c.attach("users.parquet")

# Get schema object
schema = c.schema

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
- `__len__()` - Number of fields
- `is_empty()` - Check if schema has any fields
- `__str__()` - Pretty-printed schema with all columns and types

**PySchemaField properties:**
- `name` - Column name (string)
- `data_type` - Apache Arrow data type (e.g., Int32, Utf8View, Float64)
- `nullable` - Whether column can contain NULL values (boolean)

## Data Conversion Methods

After building your data transformation pipeline, convert results to your preferred format:

```python
c = await Bundlebase.create("memory:///test_container")
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
first_name = data["name"][0]

# Option 5: Raw PyArrow batches (low-level access)
batches = await c.as_pyarrow()
for batch in batches:
    print(f"Batch with {batch.num_rows} rows")
```

**When to use each method:**

| Format | Use Case | Performance | Memory |
|--------|----------|-------------|--------|
| **to_pandas()** | Data analysis, visualization, scientific computing | Moderate | **Streaming** (constant memory) |
| **to_polars()** | Modern alternative to pandas, better performance | Excellent | **Streaming** (constant memory) |
| **to_numpy()** | NumPy array operations, machine learning | Good | Full materialization |
| **to_dict()** | Generic Python usage, JSON serialization | Good | Full materialization |
| **as_pyarrow()** | Legacy batch access (use `stream_batches()` instead) | Excellent | Full materialization |
| **stream_batches()** | **Manual batch processing for large datasets** | Excellent | **Constant memory** |

**Important notes:**
- All conversion methods are `async` - must be awaited
- Raise `ValueError` if container has no data attached
- **`to_pandas()` and `to_polars()` now use streaming internally** - safe for large datasets
- Return types:
  - `to_pandas()` → `pandas.DataFrame`
  - `to_polars()` → `polars.DataFrame`
  - `to_numpy()` → `Dict[str, numpy.ndarray]`
  - `to_dict()` → `Dict[str, list]`
  - `as_pyarrow()` → `List[pyarrow.RecordBatch]` (legacy, prefer `stream_batches()`)
  - `stream_batches()` → `AsyncIterator[pyarrow.RecordBatch]`

## Streaming API for Large Datasets

**All conversion methods now use streaming internally** to handle datasets larger than available RAM. The streaming architecture processes data in batches, maintaining constant memory usage regardless of dataset size.

### Automatic Streaming (Recommended)

For most use cases, simply use the standard conversion methods - they handle streaming automatically:

```python
# These methods now stream internally - safe for multi-GB datasets!
df = await c.to_pandas()    # Streams batches, then concatenates
df = await c.to_polars()    # Streams batches, constructs efficiently
```

**Memory footprint**: Constant per batch (~8-64MB typical), NOT proportional to total dataset size.

### Manual Batch Processing

For custom processing or maximum memory control, use `stream_batches()`:

```python
import Bundlebase

c = await Bundlebase.create()
await c.attach("huge_file.parquet")  # 10GB+ file

# Process one batch at a time - constant memory usage
total_rows = 0
async for batch in Bundlebase.stream_batches(c):
    # Each batch is a pyarrow.RecordBatch
    chunk_df = batch.to_pandas()

    # Process chunk (memory freed after iteration)
    process_chunk(chunk_df)
    total_rows += batch.num_rows

print(f"Processed {total_rows} rows with minimal memory")
```

**Use cases for manual streaming:**
- **Custom aggregations**: Incremental statistics, rolling calculations
- **Data validation**: Check each batch independently, early termination
- **Streaming writes**: Process and write batches to another system
- **Memory constraints**: Absolute control over peak memory usage

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

# Access stream metadata
schema = stream.schema  # PySchema object
```

### Streaming Performance Guidelines

**DO:**
- ✅ Use `to_pandas()` / `to_polars()` for most workflows - they stream automatically
- ✅ Use `stream_batches()` for custom incremental processing
- ✅ Process batches independently - avoid accumulating results
- ✅ Trust the streaming infrastructure for large files (10GB+)

**DON'T:**
- ❌ Don't use `as_pyarrow()` for large datasets - it materializes everything
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
- `stream_batches()` async generator yields batches one at a time
- `to_pandas()` streams batches → `pd.concat()` → single DataFrame
- `to_polars()` streams batches → constructs Polars DataFrame efficiently

**Memory profile:**
- **Old behavior** (`collect()`): `3x dataset size` (raw + Arrow + pandas)
- **New behavior** (streaming): `~50MB per batch` (constant, regardless of total size)

## Parameterized SQL and Filtering

Use parameterized queries to safely handle user input:

```python
import Bundlebase

c = await Bundlebase.create("memory:///test_container")
await c.attach("users.parquet")

# Basic filter with parameters ($1 placeholder)
filtered = await c.filter("salary > $1", [50000.0])

# Multiple parameters
filtered = await c.filter("salary > $1 AND department = $2", [50000.0, "Engineering"])

# Parameterized SQL query
results = await c.query(
    "SELECT name, salary FROM data WHERE salary > $1 AND active = $2",
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
await c.filter('country = $1', ["USA"])

# Date/timestamp parameter
await c.filter('created_at > $1', ["2023-01-01"])

# Boolean parameter
await c.filter('is_active = $1', [True])

# NULL parameter
await c.filter('notes IS NOT NULL OR special_field = $1', [None])

# Multiple types
await c.filter(
    'name = $1 AND age > $2 AND active = $3',
    ["Alice", 25, True]
)
```
