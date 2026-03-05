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
CREATE SOURCE <function> WITH (<key> = '<value>', ...) [ON <pack>]
```

See [Data Sources](../guide/sources.md) for details.

### FETCH

Discovers and attaches new files from defined sources.

```sql
FETCH [<pack> | ALL]
```

See [Data Sources](../guide/sources.md) for details.

## Connectors

Commands for managing custom source connectors. Connectors use a three-step workflow: define a connector name, configure its logic, then create a source from it.

See [Custom Source Functions](../guide/custom-sources/) for full details and SDK references.

### CREATE CONNECTOR

Declares a named connector for use with custom source functions.

```sql
CREATE CONNECTOR <name>
```

**Example:**

```sql
CREATE CONNECTOR example.connector
```

### SET CONNECTOR LOGIC

Configures how a connector runs. The logic is persisted into the bundle's commit history, making the bundle portable across environments.

```sql
SET CONNECTOR LOGIC <name> WITH (type = '<type>', logic = '<logic>' [, platform = '<platform>'])
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `type` | Yes | Source type: `lib`, `java`, `docker`, or `ipc` |
| `logic` | Yes | What to run (path to library, JAR, Docker image, or command) |
| `platform` | No | Target platform (e.g., `linux/amd64`, `darwin/arm64`, `*/*` default) |

!!! note
    The `python` type is not allowed with `SET CONNECTOR LOGIC` because Python code cannot be bundled. Use `SET TEMPORARY CONNECTOR LOGIC` instead.

**Examples:**

```sql
-- Shared library (Rust, Go, Java)
SET CONNECTOR LOGIC example.connector WITH (type = 'lib', logic = './target/release/libexample_connector.so')

-- Java JAR
SET CONNECTOR LOGIC example.connector WITH (type = 'java', logic = 'target/example-connector.jar')

-- Docker image
SET CONNECTOR LOGIC example.connector WITH (type = 'docker', logic = 'myorg/example-connector:latest')

-- IPC subprocess
SET CONNECTOR LOGIC example.connector WITH (type = 'ipc', logic = './example_connector')

-- Platform-specific
SET CONNECTOR LOGIC example.connector WITH (type = 'lib', logic = './libexample_connector.so', platform = 'linux/amd64')
```

### SET TEMPORARY CONNECTOR LOGIC

Configures how a connector runs for the current session only. The logic is **not** persisted — it exists only at runtime. Use this for Python in-process sources.

```sql
SET TEMPORARY CONNECTOR LOGIC <name> WITH (type = '<type>', logic = '<logic>' [, platform = '<platform>'])
```

**Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `type` | Yes | Source type: `python`, `lib`, `java`, `docker`, or `ipc` |
| `logic` | Yes | What to run (e.g., `module:Class` for Python, path for others) |
| `platform` | No | Target platform (default: `*/*`) |

**Examples:**

```sql
-- Python in-process (most common use case)
SET TEMPORARY CONNECTOR LOGIC example.connector WITH (type = 'python', logic = 'example_connector:ExampleConnector')

-- Any other type also works as temporary
SET TEMPORARY CONNECTOR LOGIC example.connector WITH (type = 'ipc', logic = './example_connector')
```

### DROP CONNECTOR

Removes a connector definition and all associated logic (persisted and temporary) and source instances.

```sql
DROP CONNECTOR <name>
```

**Example:**

```sql
DROP CONNECTOR example.connector
```

### DROP CONNECTOR LOGIC

Removes persisted connector logic. Optionally filter by platform.

```sql
DROP CONNECTOR LOGIC <name> [FOR PLATFORM '<platform>']
```

**Examples:**

```sql
-- Drop all persisted logic entries
DROP CONNECTOR LOGIC example.connector

-- Drop logic for a specific platform only
DROP CONNECTOR LOGIC example.connector FOR PLATFORM 'linux/amd64'
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

## Functions

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
