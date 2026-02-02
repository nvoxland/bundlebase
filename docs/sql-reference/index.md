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

## Indexes

Commands for managing search indexes.

### CREATE INDEX

Creates an index on a column.

```sql
CREATE INDEX ON <column>
```

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
