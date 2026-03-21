# Bundlebase

> Docker for data. Bundle, query with SQL, extend with Python.

Bundlebase packages data files into versioned bundles with a SQL query engine, custom functions, custom connectors, and Python/CLI interfaces.

## Docs
- [SQL Reference](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/sql-reference/index.md): Full command syntax
- [Connector SDK](https://raw.githubusercontent.com/nvoxland/bundlebase/main/sdk/python/bundlebase_sdk/README.md): Custom connectors
- [Function SDK](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/user-guide/functions.md): Custom functions
- [CLI Reference](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/cli-repl.md): CLI usage

## Quick Reference

### Installation

```
pip install bundlebase
```

### CLI Usage

```bash
# Non-interactive execute mode (agent-friendly)
bundlebase --bundle ./sales --create --execute "ATTACH 'raw_sales.csv'"
bundlebase --bundle ./sales --execute "SELECT * FROM bundle LIMIT 5" --format json
bundlebase --bundle ./sales --execute "FILTER WITH SELECT * FROM bundle WHERE amount > 0"
bundlebase --bundle ./sales --execute "COMMIT 'Cleaned sales data'"
bundlebase --bundle ./sales --execute "/schema" --format json
bundlebase --bundle ./sales --execute "/count" --format json

# Interactive REPL
bundlebase --bundle ./my-bundle --create
bundlebase --bundle ./my-bundle
bundlebase --bundle ./my-bundle --read-only
bundlebase --bundle ./my-bundle --format json   # JSON output in REPL too

# Remote bundles
bundlebase --bundle s3://mybucket/my-bundle
```

`--format`: `table` (default) or `json`. JSON mode outputs arrays of objects for queries, single values/objects for commands.
`--execute`: Run one command and exit. Errors go to stderr as `{"error": "..."}` in JSON mode. Exit code 1 on error.
Query results are hard-limited to 1000 rows. Use `LIMIT` in SQL for fewer.

REPL meta-commands: `/help`, `/show`, `/schema`, `/count`, `/status`, `/history`, `/exit`

### SQL Commands

#### Data Modification

```sql
-- Attach a data file (CSV, Parquet, JSON, etc.)
ATTACH '<path>' [TO <pack>] [WITH (<key> = <value>, ...)]

-- Remove an attached data file
DETACH '<location>'

-- Replace one attached location with another
REPLACE '<old_location>' WITH '<new_location>'

-- Filter rows using a SQL query
FILTER WITH <select_query>
```

#### Schema

```sql
-- Add a join (supports INNER, LEFT, RIGHT, FULL OUTER)
[INNER | LEFT | RIGHT | FULL [OUTER]] JOIN '<source>' AS <name> ON <condition>

-- Remove/rename a join
DROP JOIN <name>
RENAME JOIN <old_name> TO <new_name>

-- Column operations
DROP COLUMN <name>
RENAME COLUMN <old_name> TO <new_name>

-- Views (named reusable queries)
CREATE VIEW <name> AS <sql>
DROP VIEW <name>
RENAME VIEW <old_name> TO <new_name>
```

#### Sources & Fetch

```sql
-- Define a source for automatic file discovery
CREATE SOURCE <connector> WITH (<key> = '<value>', ...) [ON <pack>]

-- Discover and attach files from sources
FETCH <pack> <ADD|UPDATE|SYNC> [DRY RUN]
FETCH ALL <ADD|UPDATE|SYNC> [DRY RUN]
```

Fetch modes:
- `ADD` — attach newly discovered files only
- `UPDATE` — update already-attached files with new versions
- `SYNC` — add new + update existing + detach removed

#### Functions

```sql
-- Persistent function (not available for python runtime)
IMPORT FUNCTION <ns.name> FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]

-- Session-only function
IMPORT TEMP FUNCTION <ns.name> FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]

-- Wildcard discovery (register all functions from a library)
IMPORT FUNCTION <namespace>.* FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]

-- Management
DROP FUNCTION <ns.name> [FOR PLATFORM '<platform>']
DROP TEMP FUNCTION <ns.name> [FOR PLATFORM '<platform>']
RENAME FUNCTION <old_name> TO <new_name>
RENAME TEMP FUNCTION <old_name> TO <new_name>
DESCRIBE FUNCTION <ns.name>
```

Runtimes: `python` (temp only), `ipc`, `ffi`, `java`, `docker`.

#### Connectors

```sql
-- Persistent connector (not available for python runtime)
IMPORT CONNECTOR <name> FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]

-- Session-only connector
IMPORT TEMP CONNECTOR <name> FROM '<runtime>::<entrypoint>' [WITH (<key> = '<value>', ...)]

-- Management
DROP CONNECTOR <name> [FOR PLATFORM '<platform>']
DROP TEMP CONNECTOR <name> [FOR PLATFORM '<platform>']
RENAME CONNECTOR <old_name> TO <new_name>
RENAME TEMP CONNECTOR <old_name> TO <new_name>
DESCRIBE CONNECTOR <name>
```

Runtimes: `python` (temp only), `ipc`, `ffi`, `java`, `docker`.

#### Indexes

```sql
-- Create an index (COLUMN for filtering, TEXT for full-text search)
CREATE <COLUMN|TEXT> INDEX ON <column>

-- Management
DROP INDEX <column>
REBUILD INDEX ON <column>
REINDEX
```

#### Built-in Functions

```sql
-- Full-text search with BM25 relevance scoring
SELECT * FROM search('<index_name>', '<query>')

-- Field-specific search (for multi-column indexes)
SELECT * FROM search('product_search', 'title:learning')

-- Order by relevance
SELECT title, _score FROM search('my_search', 'machine learning') ORDER BY _score DESC
```

#### Version Control

```sql
COMMIT '<message>'
RESET
UNDO
VERIFY DATA [UPDATE]
EXPLAIN [ANALYZE] [VERBOSE] [FORMAT format] [sql]
```

EXPLAIN formats: `INDENT` (default), `TREE`, `GRAPHVIZ`.

#### Metadata

```sql
SET NAME '<name>'
SET DESCRIPTION '<description>'
SET CONFIG <key> = '<value>' [FOR '<scope>']
SAVE CONFIG <key> = '<value>' [FOR '<scope>']
```
