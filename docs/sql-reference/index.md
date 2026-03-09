# Command Syntax

Bundlebase extends standard SQL with custom commands for managing bundles. This page lists every available command organized by category.

For standard SQL queries (`SELECT`, `INSERT`, etc.), see [Querying](../guide/querying.md).

## Data Modification

Commands that change bundle data content.

### ATTACH

Adds a data file to the bundle.

```sql
ATTACH '<path>' [TO <pack>] [WITH (<key> = <value>, ...)]
```

See [Attaching Data](../guide/attaching.md) for details.

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

### FILTER

Filters the bundle's rows using a SQL query.

```sql
FILTER WITH <query>
```

See [Filtering](../guide/filtering.md) for details.

## Schema

Commands that change bundle structure.

### JOIN

Adds a join to the bundle.

```sql
[INNER | LEFT | RIGHT | FULL [OUTER]] JOIN '<source>' AS <name> ON <condition>
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

### CREATE SOURCE

Defines a source for automatic file discovery.

```sql
CREATE SOURCE <connector> WITH (<key> = '<value>', ...) [ON <pack>]
```

See [Data Sources](../guide/sources.md) for details.

### FETCH

Discovers and attaches new files from defined sources.

```sql
FETCH [<pack> | ALL]
```

See [Data Sources](../guide/sources.md) for details.

## Connectors

Commands for managing custom connectors. Connectors use a two-step workflow: create a connector with its logic, then create a source from it.

See [Custom Connectors](../guide/custom-connectors/index.md) for full details and SDK references.

### CREATE CONNECTOR

Creates a named connector with its runner and logic. The connector definition is **persisted** into the bundle's commit history.

```sql
CREATE CONNECTOR <name> WITH (runner = '<runner>', logic = '<logic>' [, platform = '<platform>'])
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `runner` | Yes | Connector runner: `lib`, `java`, `docker`, or `ipc` |
| `logic` | Yes | What to run (path to library, JAR, Docker image, or command) |
| `platform` | No | Target platform (e.g., `linux/amd64`, `darwin/arm64`, `*/*` default) |

!!! note
    The `python` runner is not allowed with `CREATE CONNECTOR` because Python code cannot be bundled. Use `CREATE TEMPORARY CONNECTOR` instead.

**Examples:**

```sql
-- Shared library (Rust, Go, Java)
CREATE CONNECTOR example.connector WITH (runner = 'lib', logic = './target/release/libexample_connector.so')

-- Java JAR
CREATE CONNECTOR example.connector WITH (runner = 'java', logic = 'target/example-connector.jar')

-- Docker image
CREATE CONNECTOR example.connector WITH (runner = 'docker', logic = 'myorg/example-connector:latest')

-- IPC subprocess
CREATE CONNECTOR example.connector WITH (runner = 'ipc', logic = './example_connector')

-- Platform-specific
CREATE CONNECTOR example.connector WITH (runner = 'lib', logic = './libexample_connector.so', platform = 'linux/amd64')
```

### CREATE TEMPORARY CONNECTOR

Creates a connector for the current session only. The logic is **not** persisted — it exists only at runtime. Use this for Python in-process sources.

```sql
CREATE TEMPORARY CONNECTOR <name> WITH (runner = '<runner>', logic = '<logic>' [, platform = '<platform>'])
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `runner` | Yes | Connector runner: `python`, `lib`, `java`, `docker`, or `ipc` |
| `logic` | Yes | What to run (e.g., `module:Class` for Python, path for others) |
| `platform` | No | Target platform (default: `*/*`) |

**Examples:**

```sql
-- Python in-process (most common use case)
CREATE TEMPORARY CONNECTOR example.connector WITH (runner = 'python', logic = 'example_connector:ExampleConnector')

-- Any other runner also works as temporary
CREATE TEMPORARY CONNECTOR example.connector WITH (runner = 'ipc', logic = './example_connector')
```

### DROP CONNECTOR

Removes a connector definition and all associated logic, or removes only logic for a specific platform.

```sql
DROP CONNECTOR <name> [FOR PLATFORM '<platform>']
```

**Examples:**

```sql
-- Drop the entire connector
DROP CONNECTOR example.connector

-- Drop logic for a specific platform only
DROP CONNECTOR example.connector FOR PLATFORM 'linux/amd64'
```

### DROP TEMPORARY CONNECTOR LOGIC

Removes runtime-only connector logic. Optionally filter by platform.

```sql
DROP TEMPORARY CONNECTOR LOGIC <name> [FOR PLATFORM '<platform>']
```

**Examples:**

```sql
-- Drop all temporary logic entries
DROP TEMPORARY CONNECTOR LOGIC example.connector

-- Drop temporary logic for a specific platform only
DROP TEMPORARY CONNECTOR LOGIC example.connector FOR PLATFORM 'linux/amd64'
```

## Indexes

Commands for managing search indexes.

### CREATE INDEX

Creates an index on a column.

```sql
CREATE <COLUMN|TEXT> INDEX ON <column>
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

### CREATE FUNCTION

Creates a named function with its logic. The function definition is **persisted** into the bundle's commit history.

```sql
CREATE FUNCTION <namespace.name>(<InputType>, ...) RETURNS <ReturnType>
  WITH (runner = '<runner>', logic = '<logic>' [, platform = '<platform>'] [, type = '<type>'])
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `runner` | Yes | Function runner: `lib`, `java`, `docker`, or `ipc` |
| `logic` | Yes | What to run. Use `path:symbol` to specify a symbol in a multi-function library (e.g., `./mylib.so:double_val`). If no `:symbol` suffix, the function's short name is used as the symbol. |
| `platform` | No | Target platform (e.g., `linux/amd64`, `darwin/arm64`, `*/*` default) |
| `type` | No | Function type: `scalar` (default) or `aggregate` |

!!! note
    The `python` runner is not allowed with `CREATE FUNCTION` because Python code cannot be bundled. Use `CREATE TEMPORARY FUNCTION` instead.

**Scalar function examples:**

```sql
-- Rust shared library (scalar) — symbol defaults to function name 'double_val'
CREATE FUNCTION acme.double_val(Int64) RETURNS Int64
  WITH (runner = 'lib', logic = './target/release/libmy_funcs.so')

-- Explicit symbol in a multi-function library
CREATE FUNCTION acme.double_val(Int64) RETURNS Int64
  WITH (runner = 'lib', logic = './target/release/libmy_funcs.so:double_val')

-- Go binary via IPC (scalar)
CREATE FUNCTION acme.to_upper(Utf8) RETURNS Utf8
  WITH (runner = 'ipc', logic = './go_funcs')

-- Java JAR (scalar)
CREATE FUNCTION acme.parse_date(Utf8) RETURNS Date32
  WITH (runner = 'java', logic = 'target/my-funcs.jar')

-- Docker image (scalar)
CREATE FUNCTION acme.geocode(Utf8) RETURNS Float64
  WITH (runner = 'docker', logic = 'myorg/geocoder:latest')

-- Platform-specific (scalar)
CREATE FUNCTION acme.double_val(Int64) RETURNS Int64
  WITH (runner = 'lib', logic = './libmy_funcs.so', platform = 'linux/amd64')
```

**Aggregate function examples:**

```sql
-- Rust shared library (aggregate)
CREATE FUNCTION acme.custom_avg(Float64) RETURNS Float64
  WITH (runner = 'lib', logic = './target/release/libmy_aggs.so', type = 'aggregate')

-- Go binary via IPC (aggregate)
CREATE FUNCTION acme.median(Int64) RETURNS Float64
  WITH (runner = 'ipc', logic = './go_aggs', type = 'aggregate')

-- Java JAR (aggregate)
CREATE FUNCTION acme.string_agg(Utf8) RETURNS Utf8
  WITH (runner = 'java', logic = 'target/my-aggs.jar', type = 'aggregate')

-- Docker image (aggregate)
CREATE FUNCTION acme.percentile(Float64) RETURNS Float64
  WITH (runner = 'docker', logic = 'myorg/stats:latest', type = 'aggregate')
```

### CREATE TEMPORARY FUNCTION

Creates a function for the current session only. The logic is **not** persisted — it exists only at runtime. Use this for Python in-process functions.

```sql
CREATE TEMPORARY FUNCTION <namespace.name>(<InputType>, ...) RETURNS <ReturnType>
  WITH (runner = '<runner>', logic = '<logic>' [, platform = '<platform>'] [, type = '<type>'])
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `runner` | Yes | Function runner: `python`, `lib`, `java`, `docker`, or `ipc` |
| `logic` | Yes | What to run (e.g., `module:function` for Python scalars, `module:ClassName` for Python aggregates) |
| `platform` | No | Target platform (default: `*/*`) |
| `type` | No | Function type: `scalar` (default) or `aggregate` |

**Scalar function examples:**

```sql
-- Python scalar function
CREATE TEMPORARY FUNCTION acme.double_val(Int64) RETURNS Int64
  WITH (runner = 'python', logic = 'my_module:double_val')

-- IPC subprocess (temporary)
CREATE TEMPORARY FUNCTION acme.to_upper(Utf8) RETURNS Utf8
  WITH (runner = 'ipc', logic = './go_funcs')
```

**Aggregate function examples:**

```sql
-- Python aggregate function (class-based)
CREATE TEMPORARY FUNCTION acme.my_sum(Int64) RETURNS Int64
  WITH (runner = 'python', logic = 'my_module:MySum', type = 'aggregate')

-- Python aggregate with multiple input types
CREATE TEMPORARY FUNCTION acme.weighted_avg(Float64, Float64) RETURNS Float64
  WITH (runner = 'python', logic = 'stats:WeightedAvg', type = 'aggregate')
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

Removes a function definition, or removes only logic for a specific platform.

```sql
DROP FUNCTION <namespace.name> [FOR PLATFORM '<platform>']
```

**Examples:**

```sql
-- Drop the entire function
DROP FUNCTION acme.double_val

-- Drop logic for a specific platform only
DROP FUNCTION acme.double_val FOR PLATFORM 'linux/amd64'
```

### DROP TEMPORARY FUNCTION

Removes runtime-only function logic. Optionally filter by platform.

```sql
DROP TEMPORARY FUNCTION <namespace.name> [FOR PLATFORM '<platform>']
```

### CREATE FUNCTIONS FROM

Discovers and registers all functions exported by a shared library or IPC executable in a single command. Uses the manifest discovery protocol.

```sql
CREATE FUNCTIONS FROM '<path>' WITH (runner = '<runner>', namespace = '<namespace>' [, platform = '<platform>'])
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `runner` | Yes | Discovery method: `lib` (calls `bundlebase_functions()` C symbol) or `ipc` (runs `path --bundlebase-functions`) |
| `namespace` | Yes | Namespace for registered functions (e.g., `acme`) |
| `platform` | No | Target platform (default: `*/*`) |

**Examples:**

```sql
-- Register all functions from a Rust shared library
CREATE FUNCTIONS FROM './target/release/libmy_funcs.so' WITH (runner = 'lib', namespace = 'acme')

-- Register functions from an IPC executable
CREATE FUNCTIONS FROM './my_go_funcs' WITH (runner = 'ipc', namespace = 'tools')

-- Platform-specific library
CREATE FUNCTIONS FROM './libmy_funcs.so' WITH (runner = 'lib', namespace = 'acme', platform = 'linux/amd64')
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

Each discovered function is registered as if individually created with `CREATE FUNCTION`. Functions from a bulk-created set can be dropped individually with `DROP FUNCTION`.

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

Reverts the last committed change.

```sql
UNDO
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
