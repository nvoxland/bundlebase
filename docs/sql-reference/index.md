# Command Syntax

Bundlebase extends standard SQL with custom commands for managing bundles. This page lists every available command organized by category.

For standard SQL queries (`SELECT`, `INSERT`, etc.), see [Querying](../guide/querying.md).

## String Literals

Bundlebase supports three string literal forms:

| Form | Example | Notes |
|------|---------|-------|
| Single-quoted | `'hello world'` | Standard SQL. Use `''` to escape a single quote inside: `'it''s fine'`. Supports `\n`, `\r`, `\t` escape sequences. |
| Double-quoted | `"hello world"` | Same escape rules as single-quoted. Typically used for identifiers in standard SQL, but accepted as a string value here too. |
| Dollar-quoted | `$$hello world$$` | PostgreSQL-style. Content is **raw** — no escaping needed. Ideal for multi-line strings, JSON, and any text containing single quotes. |

Dollar-quoted strings are particularly useful when the value contains quotes or spans multiple lines:

```sql
-- Single quotes inside need no escaping
COMMIT $$fixed the 'broken' connector$$

-- Multi-line JSON body for an HTTP POST source
CREATE SOURCE USING http WITH (
    url = 'https://api.example.com/query',
    method = 'POST',
    body = $${
        "filter": {"state": "MN", "type": "Lake"},
        "format": "csv"
    }$$
)
```

## Data Modification

Commands that change bundle data content.

### ATTACH

Adds a data file to the bundle. Supports CSV, TSV, Parquet, JSON files, and `bundle://` URLs to reference other bundles.

```sql
ATTACH '<path>' [TO <pack>] [WITH (<key> = <value>, ...)]
```

**Examples:**

```sql
-- Attach a local file
ATTACH 'data.csv'

-- Attach another bundle's query output (includes all its filters, column ops, etc.)
ATTACH 'bundle:///path/to/other/bundle'

-- Attach a remote bundle
ATTACH 'bundle+s3://bucket/path/to/bundle'
```

See [Attaching Data](../guide/attaching.md) for details.

### IMPORT JOIN

Solidifies an existing `bundle://` join by copying all commits, data files, indexes, and connectors/functions from the source bundle into the target. The join must have been created with `JOIN 'bundle://...'`.

```sql
IMPORT JOIN <name> [FLATTEN HISTORY]
```

**Examples:**

```sql
-- First, create a join referencing another bundle
JOIN 'bundle://./stations' AS stations ON lake_id = stations.lake_id

-- Then solidify it — copies all data and history into this bundle
IMPORT JOIN stations

-- Or flatten all imported commits into one
IMPORT JOIN stations FLATTEN HISTORY
```

### DETACH

Removes an attached data file from the bundle.

```sql
DETACH '<location>'
```

See [Attaching Data](../guide/attaching.md) for details.

### REPLACE

Replaces one attached location with another.

```sql
REPLACE '<old_location>' WITH '<new_location>'
```

See [Attaching Data](../guide/attaching.md) for details.

### DELETE

Deletes rows matching a WHERE condition. Deleted rows are tracked via tombstone files and excluded from all subsequent queries. Tombstone files are written on COMMIT.

```sql
DELETE FROM bundle WHERE <condition>
```

**Examples:**

```sql
-- Remove negative sentinel values
DELETE FROM bundle WHERE depth_m < 0

-- Remove inactive records
DELETE FROM bundle WHERE status = 'inactive' AND last_login < '2020-01-01'
```

**Python API:**

```python
c = await c.delete("depth_m < 0")
await c.commit("Removed sentinel values")
```

### UPDATE

Updates rows matching a WHERE condition with new values. Updated values are stored in overlay parquet files and merged at query time. Original data files are not modified.

SET expressions can reference other columns and use SQL functions.

```sql
UPDATE bundle SET <col> = <expr> [, <col> = <expr> ...] WHERE <condition>
```

**Examples:**

```sql
-- Adjust salaries
UPDATE bundle SET salary = salary * 1.1 WHERE department = 'eng'

-- Set values to NULL
UPDATE bundle SET status = NULL WHERE inactive = true

-- Update multiple columns
UPDATE bundle SET name = 'unknown', age = 0 WHERE name IS NULL
```

**Python API:**

```python
c = await c.update("SET salary = salary * 1.1 WHERE department = 'eng'")
await c.commit("Adjusted engineering salaries")
```

### ALWAYS DELETE

Registers a persistent delete rule that automatically applies to all future ATTACH operations. Also immediately deletes matching rows from current data.

Multiple rules can be added and they accumulate.

```sql
ALWAYS DELETE FROM bundle WHERE <condition>
```

**Examples:**

```sql
-- Always remove negative sentinel values from new data
ALWAYS DELETE FROM bundle WHERE depth_m < 0

-- Always remove inactive records
ALWAYS DELETE FROM bundle WHERE status = 'inactive'
```

**Python API:**

```python
c = await c.always_delete("depth_m < 0")
await c.commit("Add data quality rule")
```

### DROP ALWAYS DELETE

Removes always-delete rules. Without a WHERE clause, removes all rules.

```sql
-- Remove a specific rule
DROP ALWAYS DELETE WHERE depth_m < 0

-- Remove all rules
DROP ALWAYS DELETE
```

**Python API:**

```python
# Remove specific rule
c = await c.drop_always_delete("depth_m < 0")

# Remove all rules
c = await c.drop_always_delete()
```

### SHOW ALWAYS DELETES

Lists all active always-delete rules. Also available as `SELECT * FROM bundle_info.always_deletes`.

```sql
SHOW ALWAYS DELETES
```

### ALWAYS UPDATE

Registers a persistent update rule that automatically applies to all future ATTACH operations. Also immediately updates matching rows in current data.

Multiple rules can be added and they accumulate.

```sql
ALWAYS UPDATE bundle SET <col> = <expr> [, ...] WHERE <condition>
```

**Examples:**

```sql
-- Always normalize negative depths to zero
ALWAYS UPDATE bundle SET depth_m = 0 WHERE depth_m < 0

-- Always uppercase status values
ALWAYS UPDATE bundle SET status = upper(status) WHERE status IS NOT NULL
```

**Python API:**

```python
c = await c.always_update("SET depth_m = 0 WHERE depth_m < 0")
await c.commit("Add data quality rule")
```

### DROP ALWAYS UPDATE

Removes always-update rules. Without a SET/WHERE clause, removes all rules.

```sql
-- Remove a specific rule
DROP ALWAYS UPDATE SET depth_m = 0 WHERE depth_m < 0

-- Remove all rules
DROP ALWAYS UPDATE
```

**Python API:**

```python
# Remove specific rule
c = await c.drop_always_update("SET depth_m = 0 WHERE depth_m < 0")

# Remove all rules
c = await c.drop_always_update()
```

### SHOW ALWAYS UPDATES

Lists all active always-update rules. Also available as `SELECT * FROM bundle_info.always_updates`.

```sql
SHOW ALWAYS UPDATES
```

### FILTER

Filters the bundle's rows using a SQL query.

```sql
FILTER WITH <query>
```

See [Filtering](../guide/filtering.md) for details.

## Schema

Commands that change bundle structure.

### JOIN

Adds a join to the bundle. The source can be a file or a `bundle://` URL to join with another bundle's query output.

```sql
[INNER | LEFT | RIGHT | FULL [OUTER]] JOIN '<source>' AS <name> ON <condition>
```

**Examples:**

```sql
-- Join with a local file
JOIN 'orders.csv' AS orders ON id = orders.customer_id

-- Join with another bundle (includes all its filters, column ops, etc.)
JOIN 'bundle:///path/to/other/bundle' AS other ON id = other.id

-- Join with a remote bundle
LEFT JOIN 'bundle+s3://bucket/regions' AS regions ON region_code = regions.code
```

See [Joins](../guide/joins.md) for details.

### DROP JOIN

Removes a join from the bundle.

```sql
DROP JOIN <name>
```

See [Joins](../guide/joins.md) for details.

### RENAME JOIN

Renames an existing join.

```sql
RENAME JOIN <old_name> TO <new_name>
```

See [Joins](../guide/joins.md) for details.

### DROP COLUMN

Removes a column from the bundle.

```sql
DROP COLUMN <name>
```

See [Columns](../guide/columns.md) for details.

### RENAME COLUMN

Renames an existing column.

```sql
RENAME COLUMN <old_name> TO <new_name>
```

See [Columns](../guide/columns.md) for details.

### CREATE VIEW

Creates a named, reusable query.

```sql
CREATE VIEW <name> AS <sql>
```

See [Views](../guide/views.md) for details.

### DROP VIEW

Removes a view from the bundle.

```sql
DROP VIEW <name>
```

See [Views](../guide/views.md) for details.

### RENAME VIEW

Renames an existing view.

```sql
RENAME VIEW <old_name> TO <new_name>
```

See [Views](../guide/views.md) for details.

## Sources

Commands for managing data sources.

### TEST CONNECTOR

Tests a connector without creating a source. Calls discover() and data() to validate the integration, showing discovered locations, schema, and sample data.

```sql
TEST CONNECTOR <name> [WITH (<key> = '<value>', ...)]
TEST TEMP CONNECTOR '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]
```

See [Custom Connectors](../guide/custom-connectors/index.md) for details.

### CREATE SOURCE

Defines a source for automatic file discovery.

```sql
CREATE SOURCE [FOR <pack>] USING <connector> [WITH (<key> = '<value>', ...)] [SAVE AS <AUTO|COPY|PARQUET|REF>] [MIN BATCH <size>]
```

Use `MIN BATCH` with a human-readable size such as `15M` or `3G` when you want small fetched files merged into larger parquet batches.

See [Data Sources](../guide/sources.md) for details.

### FETCH

Discovers and attaches new files from defined sources.

```sql
FETCH <pack> <ADD|UPDATE|SYNC> [DRY RUN]
FETCH ALL <ADD|UPDATE|SYNC> [DRY RUN]
```

The optional `DRY RUN` modifier shows what would be added, updated, or removed without actually executing any changes. This is useful for previewing the effect of a fetch before committing to it.

**Examples:**

```sql
-- Preview what a fetch would do
FETCH base ADD DRY RUN

-- Preview a full sync across all sources
FETCH ALL SYNC DRY RUN

-- Execute the fetch (no DRY RUN)
FETCH base ADD
```

See [Data Sources](../guide/sources.md) for details.

## Connectors

Commands for managing custom connectors. Connectors use a two-step workflow: create a connector, then create a source from it.

See [Custom Connectors](../guide/custom-connectors/index.md) for full details and SDK references.

### IMPORT CONNECTOR

Creates a named connector. The connector definition is **persisted** into the bundle's commit history.

```sql
IMPORT CONNECTOR <name> FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]
```

The `runtime::entrypoint` URI specifies both the runtime and what to run. An optional `WITH` clause can provide additional parameters like `platform`.

!!! note
    The `python` runtime is not allowed with `IMPORT CONNECTOR` because Python code cannot be bundled. Use `IMPORT TEMP CONNECTOR` instead.

**Examples:**

```sql
-- Shared library (Rust, Go, Java)
IMPORT CONNECTOR example.connector FROM 'ffi::./target/release/libexample_connector.so'

-- Java JAR
IMPORT CONNECTOR example.connector FROM 'java::target/example-connector.jar'

-- Docker image
IMPORT CONNECTOR example.connector FROM 'docker::myorg/example-connector:latest'

-- IPC subprocess
IMPORT CONNECTOR example.connector FROM 'ipc::./example_connector'

-- Platform-specific
IMPORT CONNECTOR example.connector FROM 'ffi::./libexample_connector.so' WITH (platform = 'linux/amd64')
```

### IMPORT TEMP CONNECTOR

Creates a connector for the current session only. The entrypoint is **not** persisted — it exists only at runtime. Use this for Python in-process sources.

```sql
IMPORT TEMP CONNECTOR <name> FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]
```

**Examples:**

```sql
-- Python in-process (most common use case)
IMPORT TEMP CONNECTOR example.connector FROM 'python::example_connector:ExampleConnector'

-- Any other runtime also works as temporary
IMPORT TEMP CONNECTOR example.connector FROM 'ipc::./example_connector'
```

### DROP CONNECTOR

Removes a connector, or removes a connector for a specific platform only.

```sql
DROP CONNECTOR <name> [FOR PLATFORM '<platform>']
```

**Examples:**

```sql
-- Drop the entire connector
DROP CONNECTOR example.connector

-- Drop connector for a specific platform only
DROP CONNECTOR example.connector FOR PLATFORM 'linux/amd64'
```

### DROP TEMP CONNECTOR

Removes runtime-only connector entries. Optionally filter by platform.

```sql
DROP TEMP CONNECTOR <name> [FOR PLATFORM '<platform>']
```

**Examples:**

```sql
-- Drop all temporary connector entries
DROP TEMP CONNECTOR example.connector

-- Drop temporary connector for a specific platform only
DROP TEMP CONNECTOR example.connector FOR PLATFORM 'linux/amd64'
```

### RENAME CONNECTOR

Rename a connector definition to a new dotted name. All entries (platform variants) are renamed. Sources referencing the old connector name are automatically updated.

**Syntax:**
```sql
RENAME CONNECTOR <old_name> TO <new_name>
```

**Example:**
```sql
RENAME CONNECTOR acme.weather TO acme.weather_v2
```

### RENAME TEMP CONNECTOR

Rename temporary (session-only) connector entries to a new name. Only temporary entries are renamed; persistent entries are not affected.

**Syntax:**
```sql
RENAME TEMP CONNECTOR <old_name> TO <new_name>
```

**Example:**
```sql
RENAME TEMP CONNECTOR acme.weather TO acme.weather_v2
```

### DESCRIBE CONNECTOR

Returns metadata about a registered connector. Shows all entries matching the given name, including runtime, entrypoint, platform, and whether the entry is temporary.

```sql
DESCRIBE CONNECTOR <dotted_name>
```

The result is a table with the following columns:

| Column | Type | Description |
|--------|------|-------------|
| `name` | string | Connector name |
| `runtime` | string | Runtime type (e.g., `ipc`, `ffi`, `python`) |
| `entrypoint` | string | Entrypoint path or module reference |
| `platform` | string | Target platform (e.g., `*/*`, `linux/amd64`) |
| `temporary` | boolean | Whether the entry is runtime-only |

**Example:**

```sql
DESCRIBE CONNECTOR acme.weather
```

```
+---------------+---------+---------------------+----------+-----------+
| name          | runtime | entrypoint          | platform | temporary |
+---------------+---------+---------------------+----------+-----------+
| acme.weather  | ipc     | ./my_connector      | */*      | false     |
| acme.weather  | python  | my_module:MyWeather | */*      | true      |
+---------------+---------+---------------------+----------+-----------+
```

## Indexes

Commands for managing search indexes.

### CREATE INDEX

Creates an index on a column.

```sql
CREATE <BTREE|TEXT> INDEX ON <column>
```

!!! note
    The SQL syntax supports single-column indexes only. For multi-column text indexes, use the Python API: `bundle.create_index(["col1", "col2"], "text")`.

See [Indexing](../guide/indexing.md) for details.

### DROP INDEX

Removes an index from a column.

```sql
DROP INDEX <column>
```

See [Indexing](../guide/indexing.md) for details.

### REBUILD INDEX

Rebuilds an index on a column.

```sql
REBUILD INDEX ON <column>
```

See [Indexing](../guide/indexing.md) for details.

### REINDEX

Rebuilds all indexes, or a specific one.

```sql
REINDEX
```

See [Indexing](../guide/indexing.md) for details.

## User-Defined Functions

Commands for creating custom SQL functions that can be used in queries.

### IMPORT FUNCTION

Creates a named function. The function definition is **persisted** into the bundle's commit history. Input types, return type, and function kind (scalar/aggregate) are auto-detected from the runtime.

```sql
IMPORT FUNCTION <namespace.name> FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]
```

An optional `WITH` clause can provide additional parameters like `platform`.

!!! note
    The `python` runtime is not allowed with `IMPORT FUNCTION` because Python code cannot be bundled. Use `IMPORT TEMP FUNCTION` instead.

**Scalar function examples:**

```sql
-- Rust shared library (scalar) — symbol defaults to function name 'double_val'
IMPORT FUNCTION acme.double_val FROM 'ffi::./target/release/libmy_funcs.so'

-- Explicit symbol in a multi-function library
IMPORT FUNCTION acme.double_val FROM 'ffi::./target/release/libmy_funcs.so:double_val'

-- Go binary via IPC (scalar)
IMPORT FUNCTION acme.to_upper FROM 'ipc::./go_funcs'

-- Java JAR (scalar)
IMPORT FUNCTION acme.parse_date FROM 'java::target/my-funcs.jar'

-- Docker image (scalar)
IMPORT FUNCTION acme.geocode FROM 'docker::myorg/geocoder:latest'

-- Platform-specific (scalar)
IMPORT FUNCTION acme.double_val FROM 'ffi::./libmy_funcs.so' WITH (platform = 'linux/amd64')
```

**Aggregate function examples:**

```sql
-- Rust shared library (aggregate)
IMPORT FUNCTION acme.custom_avg FROM 'ffi::./target/release/libmy_aggs.so'

-- Go binary via IPC (aggregate)
IMPORT FUNCTION acme.median FROM 'ipc::./go_aggs'

-- Java JAR (aggregate)
IMPORT FUNCTION acme.string_agg FROM 'java::target/my-aggs.jar'

-- Docker image (aggregate)
IMPORT FUNCTION acme.percentile FROM 'docker::myorg/stats:latest'
```

### IMPORT TEMP FUNCTION

Creates a function for the current session only. The entrypoint is **not** persisted — it exists only at runtime. Use this for Python in-process functions. Input types, return type, and function kind are auto-detected.

```sql
IMPORT TEMP FUNCTION <namespace.name> FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]
```

**Scalar function examples:**

```sql
-- Python scalar function
IMPORT TEMP FUNCTION acme.double_val FROM 'python::my_module:double_val'

-- IPC subprocess (temporary)
IMPORT TEMP FUNCTION acme.to_upper FROM 'ipc::./go_funcs'
```

**Aggregate function examples:**

```sql
-- Python aggregate function (class-based)
IMPORT TEMP FUNCTION acme.my_sum FROM 'python::my_module:MySum'

-- Python aggregate with multiple input types
IMPORT TEMP FUNCTION acme.weighted_avg FROM 'python::stats:WeightedAvg'
```

**Python scalar function interface:**

```python
# my_module.py
import pyarrow as pa
import pyarrow.compute as pc

def double_val(col: pa.Array) -> pa.Array:
    """Receives PyArrow arrays, returns a PyArrow array."""
    return pc.multiply(col, 2)
```

**Python aggregate function interface:**

```python
# my_module.py
import pyarrow as pa
import pyarrow.compute as pc

class MySum:
    def create_state(self):
        """Return initial accumulator state as a PyArrow scalar."""
        return pa.scalar(0, type=pa.int64())

    def accumulate(self, state, values):
        """Accumulate a batch into state. Returns updated state scalar."""
        return pa.scalar(state.as_py() + pc.sum(values).as_py(), type=pa.int64())

    def merge(self, state1, state2):
        """Merge two states (for parallel execution)."""
        return pa.scalar(state1.as_py() + state2.as_py(), type=pa.int64())

    def evaluate(self, state):
        """Produce final result from state."""
        return state
```

**Using aggregate functions in queries:**

```sql
-- Basic aggregation
SELECT acme.my_sum(amount) FROM bundle

-- With GROUP BY
SELECT category, acme.my_sum(amount) FROM bundle GROUP BY category

-- As a window function (any aggregate works with OVER)
SELECT id, acme.my_sum(amount) OVER (ORDER BY id) as running_total FROM bundle

-- With window partitioning
SELECT category, id, acme.my_sum(amount) OVER (PARTITION BY category ORDER BY id) FROM bundle
```

### DROP FUNCTION

Removes a function definition, or removes only the entrypoint for a specific platform.

```sql
DROP FUNCTION <namespace.name> [FOR PLATFORM '<platform>']
```

**Examples:**

```sql
-- Drop the entire function
DROP FUNCTION acme.double_val

-- Drop function for a specific platform only
DROP FUNCTION acme.double_val FOR PLATFORM 'linux/amd64'
```

### DROP TEMP FUNCTION

Removes runtime-only function entries. Optionally filter by platform.

```sql
DROP TEMP FUNCTION <namespace.name> [FOR PLATFORM '<platform>']
```

### RENAME FUNCTION

Rename a function definition to a new dotted name. All overloads are renamed. Old UDFs are deregistered and re-registered under the new name.

**Syntax:**
```sql
RENAME FUNCTION <old_name> TO <new_name>
```

**Example:**
```sql
RENAME FUNCTION acme.double_val TO acme.double_val_v2
```

### RENAME TEMP FUNCTION

Rename temporary (session-only) function entries to a new name. Only temporary entries are renamed; persistent entries are not affected.

**Syntax:**
```sql
RENAME TEMP FUNCTION <old_name> TO <new_name>
```

**Example:**
```sql
RENAME TEMP FUNCTION acme.double_val TO acme.double_val_v2
```

### DESCRIBE FUNCTION

Returns metadata about a registered function. Shows all entries matching the given name, including kind, input/return types, runtime, entrypoint, platform, and whether the entry is temporary.

```sql
DESCRIBE FUNCTION <dotted_name>
```

The result is a table with the following columns:

| Column | Type | Description |
|--------|------|-------------|
| `name` | string | Function name |
| `kind` | string | Function kind (`scalar`, `aggregate`, `table_valued`) |
| `input_types` | string | Comma-separated Arrow input types (e.g., `Int64, Utf8`) |
| `return_type` | string | Arrow return type (e.g., `Int64`) |
| `runtime` | string | Runtime type (e.g., `ipc`, `ffi`, `python`) |
| `entrypoint` | string | Entrypoint path or module reference |
| `platform` | string | Target platform (e.g., `*/*`, `linux/amd64`) |
| `temporary` | boolean | Whether the entry is runtime-only |

**Example:**

```sql
DESCRIBE FUNCTION acme.double_val
```

```
+------------------+--------+-------------+-------------+---------+-------------------------+----------+-----------+
| name             | kind   | input_types | return_type | runtime | entrypoint              | platform | temporary |
+------------------+--------+-------------+-------------+---------+-------------------------+----------+-----------+
| acme.double_val  | scalar | Int64       | Int64       | ipc     | ./my_funcs              | */*      | false     |
| acme.double_val  | scalar | Int64       | Int64       | python  | my_module:double_val    | */*      | true      |
+------------------+--------+-------------+-------------+---------+-------------------------+----------+-----------+
```

### DESCRIBE DATA

Returns per-column statistics for specified columns: min, max, average, null counts, most frequent values, and values that fail to cast to an expected type.

```sql
DESCRIBE DATA IN <col1> [AS <type>], <col2> [AS <type>], ...
```

The optional `AS <type>` specifies an expected SQL data type (e.g., `BIGINT`, `DOUBLE`, `VARCHAR`). When provided, the `top_10_invalid` column shows values that fail `TRY_CAST` to that type — useful for detecting sentinel values like `-99` or `N/A` in data that should be numeric.

The result is a table with one row per column:

| Column | Type | Description |
|--------|------|-------------|
| `column` | string | Column name |
| `data_type` | string | Actual Arrow data type |
| `min` | string | Minimum value (null for text columns) |
| `max` | string | Maximum value (null for text columns) |
| `avg` | string | Average value (null for text and date columns) |
| `num_nulls` | integer | Count of null values |
| `num_not_nulls` | integer | Count of non-null values |
| `top_10_values` | string | JSON array of `[{"value": "x", "count": 5}, ...]` |
| `top_10_invalid` | string | JSON array of values that fail to cast (only when `AS <type>` specified) |

**Examples:**

```sql
-- Basic column profiling
DESCRIBE DATA IN salary, first_name

-- Detect sentinel values: find non-numeric values in a column expected to be DOUBLE
DESCRIBE DATA IN secchi_depth_m AS DOUBLE, station_name

-- Check for values that can't be cast to integer
DESCRIBE DATA IN id AS BIGINT
```

**Python API:**

```python
# Simple column list
result = await bundle.describe_data(["salary", "first_name"])
df = await result.to_pandas()

# With expected type for sentinel detection
result = await bundle.describe_data([("secchi_depth_m", "DOUBLE"), "station_name"])
df = await result.to_pandas()
```

### Wildcard Function Discovery

Discovers and registers all functions exported by a shared library or IPC executable in a single command using the wildcard `namespace.*` syntax. Uses the manifest discovery protocol.

```sql
IMPORT FUNCTION <namespace>.* FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]
```

**Examples:**

```sql
-- Register all functions from a Rust shared library
IMPORT FUNCTION acme.* FROM 'ffi::./target/release/libmy_funcs.so'

-- Register functions from an IPC executable
IMPORT FUNCTION tools.* FROM 'ipc::./my_go_funcs'

-- Platform-specific library
IMPORT FUNCTION acme.* FROM 'ffi::./libmy_funcs.so' WITH (platform = 'linux/amd64')
```

**Manifest format:** Libraries and executables must return a JSON manifest:

```json
{"functions": [
  {"name": "double_val", "symbol": "double_val",
   "input_types": ["Int64"], "return_type": "Int64", "kind": "scalar"},
  {"name": "my_sum", "input_types": ["Int64"],
   "return_type": "Int64", "kind": "aggregate"}
]}
```

Each discovered function is registered as if individually created with `IMPORT FUNCTION`. Functions from a bulk-discovered set can be dropped individually with `DROP FUNCTION`.

## Built-in Functions

Functions available in SQL queries.

### SEARCH

Table function for full-text search against a named text index. Returns matching rows with a BM25 relevance `_score` column.

```sql
SELECT * FROM search('<index_name>', '<query>')
```

**Parameters:**

- `index_name` — The name of the text index (created with `create_index()`)
- `query` — The search query string using [Tantivy query syntax](https://docs.rs/tantivy/latest/tantivy/query/struct.QueryParser.html)

**Examples:**

```sql
-- Basic search
SELECT * FROM search('my_search', 'machine learning')

-- Order by relevance score
SELECT title, description, _score FROM search('my_search', 'machine learning') ORDER BY _score DESC

-- Field-specific search (for multi-column indexes)
SELECT * FROM search('product_search', 'title:learning')

-- Additional filters on top of search results
SELECT * FROM search('my_search', 'machine learning') WHERE category = 'AI'
```

See [Text Search](../guide/text-search.md) for details on creating text indexes and available tokenizers.

## Version Control

Commands for bundle versioning.

### COMMIT

Saves all pending changes as a new version.

```sql
COMMIT '<message>'
```

See [Versioning](../guide/versioning.md) for details.

### RESET

Discards all uncommitted changes.

```sql
RESET
```

See [Versioning](../guide/versioning.md) for details.

### UNDO

Reverts the last uncommitted change, keeping earlier uncommitted changes intact. Use `UNDO LAST N` to undo multiple changes at once. Errors if N exceeds the number of uncommitted changes.

```sql
UNDO
UNDO LAST 3
```

See [Versioning](../guide/versioning.md) for details.

### VERIFY DATA

Verifies the integrity of attached data. Use `UPDATE` to fix issues.

```sql
VERIFY DATA [UPDATE]
```

See [Versioning](../guide/versioning.md) for details.

### EXPLAIN

Shows the query execution plan for the bundle's dataframe or a given SQL statement.

```sql
EXPLAIN [ANALYZE] [VERBOSE] [FORMAT format] [sql]
```

**Options:**

- `ANALYZE` — Run the plan and show actual execution statistics
- `VERBOSE` — Show more detailed plan information
- `FORMAT format` — Output format: `INDENT` (default), `TREE`, or `GRAPHVIZ`
- `sql` — Optional SQL statement to explain (if omitted, explains the bundle's dataframe)

**Examples:**

```sql
EXPLAIN
EXPLAIN ANALYZE
EXPLAIN VERBOSE FORMAT TREE
EXPLAIN SELECT * FROM bundle WHERE id > 10
EXPLAIN ANALYZE FORMAT TREE SELECT * FROM bundle WHERE salary > 50000
```

## Metadata

Commands for bundle metadata.

### SET NAME

Sets the bundle's display name.

```sql
SET NAME '<name>'
```

See [Metadata](../guide/metadata.md) for details.

### SET DESCRIPTION

Sets the bundle's description.

```sql
SET DESCRIPTION '<description>'
```

See [Metadata](../guide/metadata.md) for details.

### SET CONFIG

Sets a runtime configuration value for the current session only (not persisted). Takes the highest priority, overriding all other config sources. Works on both read-only bundles and builders.

```sql
SET CONFIG <key> = '<value>' [FOR '<scope>']
```

See [Configuration](../guide/configuration.md) for details.

### SAVE CONFIG

Saves a configuration value to the bundle manifest, optionally scoped to a scope (URL prefix or alias name).

```sql
SAVE CONFIG <key> = '<value>' [FOR '<scope>']
```

See [Metadata](../guide/metadata.md) and [Configuration](../guide/configuration.md) for details.

## Export

### EXPORT DATA

Exports query results to a file. The output format is determined by the file extension.

```sql
EXPORT DATA TO '<path>' <sql>
```

**Supported formats:**

| Extension | Format |
|-----------|--------|
| `.csv` | Comma-separated values |
| `.jsonl` | JSON Lines (one JSON object per line) |

**Examples:**

```sql
-- Export all data to CSV
EXPORT DATA TO 'output.csv' SELECT * FROM bundle

-- Export filtered results to JSON Lines
EXPORT DATA TO '/tmp/active_users.jsonl' SELECT * FROM bundle WHERE active = true

-- Export aggregated results
EXPORT DATA TO 'summary.csv' SELECT department, COUNT(*) as cnt, AVG(salary) as avg_sal FROM bundle GROUP BY department
```

### EXPORT HOLLOW

Creates a "hollow" bundle at the target path — containing source definitions,
always-update/always-delete rules, column operations, and structure, but no
attached data. Recipients can open the hollow bundle and run `FETCH` to pull
the raw data themselves.

```sql
EXPORT HOLLOW TO '<path>'
```

The target path supports `.tar` files for a portable single-file bundle.

**Examples:**

```sql
-- Export to a directory
EXPORT HOLLOW TO 'path/to/hollow'

-- Export to a tar file
EXPORT HOLLOW TO 'path/to/hollow.tar'
```

**Behavior:**

- Strips all data operations: `ATTACH`, `DETACH`, `REPLACE`, `DELETE`, `UPDATE`, `INDEX`
- Preserves: `CREATE SOURCE`, `ALWAYS DELETE`, `ALWAYS UPDATE`, `RENAME COLUMN`,
  `CAST COLUMN`, `ADD COLUMN`, `DROP COLUMN`, `FILTER`, `JOIN`, views
- Fills the `EXPECTED SCHEMA` on each `CREATE SOURCE` from the last-seen fetched schema
- The hollow bundle has no rows and no schema until `FETCH` is run
- History is reset to a single "Hollow export" commit

## Help & Introspection

### SHOW

Shows the contents of a bundle metadata table. A shortcut for `SELECT * FROM bundle_info.<table>`.

```sql
SHOW <table>
```

Available tables: `DETAILS`, `HISTORY`, `STATUS`, `VIEWS`, `INDEXES`, `PACKS`, `BLOCKS`, `CONFIG`, `COMMANDS`, `CONNECTORS`, `FUNCTIONS`.

**Examples:**

```sql
SHOW HISTORY
SHOW STATUS
SHOW DETAILS
SHOW CONFIG
SHOW COMMANDS
SHOW CONNECTORS
```

### SYNTAX

Shows syntax and usage information for bundlebase commands. With no arguments, lists all available commands. With a command name, shows detailed syntax and examples.

```sql
SYNTAX [<command>]
```

**Examples:**

```sql
-- List all available commands
SYNTAX

-- Show detailed syntax for a specific command
SYNTAX ATTACH
SYNTAX IMPORT CONNECTOR
SYNTAX FETCH
```
