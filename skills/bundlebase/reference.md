# Bundlebase

> Docker for data. Bundle, query with SQL, extend with Python.

Bundlebase packages data files into versioned bundles with a SQL query engine, custom functions, custom connectors, and Python/CLI interfaces.

## Docs
- [SQL Reference](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/sql-reference/index.md): Full command syntax
- [Sources & Connectors](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/sources.md): Built-in connectors (Kaggle, S3, FTP, PostgreSQL, etc.)
- [Custom Connectors](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/index.md): Build your own data connectors
- [Python Connector SDK](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/python.md): Python SDK for connectors
- [Functions](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/functions.md): Custom SQL functions
- [CLI Reference](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/cli-repl.md): CLI usage

## Quick Reference

### Installation

```
pip install bundlebase
```

### CLI Usage

```bash
# Read-only queries (<<< passes SQL via stdin, avoids shell quoting issues)
bundlebase query --bundle ./sales --format json <<< "SELECT * FROM bundle LIMIT 5"
bundlebase query --bundle ./sales --format json <<< "SHOW COLUMNS"
bundlebase query --bundle ./sales --format json <<< "SHOW COUNT"

# Create a new bundle
bundlebase create --bundle ./sales <<< "ATTACH 'raw_sales.csv'"

# Mutating commands on existing bundles (auto-commits after each call)
bundlebase extend --bundle ./sales <<< "FILTER WITH SELECT * FROM bundle WHERE amount > 0"
bundlebase extend --bundle ./sales -m "Cleaned sales data" <<< "RENAME COLUMN amt TO amount"

# Multiple statements in one call (committed together)
bundlebase extend --bundle ./sales -m "Initial cleanup" <<< "DROP COLUMN temp_id; RENAME COLUMN amt TO amount"

# Interactive REPL
bundlebase repl --bundle ./my-bundle
bundlebase repl --bundle ./my-bundle --read-only

# Remote bundles
bundlebase query --bundle s3://mybucket/my-bundle --format json <<< "SELECT * FROM bundle LIMIT 5"
```

`--format`: `table` (default) or `json`. JSON mode outputs arrays of objects for queries, single values/objects for commands.
`bundlebase execute` is an alias for `bundlebase extend`.
`bundlebase extend` auto-commits after each call. Use `-m` for a custom commit message.
`bundlebase extend --to ./new-dir` extends to a new directory instead of modifying in place.
Multiple statements can be separated with `;` — all are validated before any execute, and all changes commit together.
Query results are hard-limited to 1000 rows. Use `LIMIT` in SQL for fewer.

REPL meta-commands: `/help`, `/clear`, `/exit`

### MCP Usage

MCP mode keeps the bundle open across calls — use for multi-step workflows.

```bash
# Start as MCP server (configure in your AI assistant's MCP settings)
bundlebase mcp --bundle ./my-bundle
bundlebase mcp --bundle ./my-bundle --read-only
```

MCP tools: `query` (any SQL/command), `schema`, `count`, `sample`, `status`, `history`.

Prefer CLI (`bundlebase query`) for one-shot queries. Prefer MCP for building bundles or multi-step exploration.

### SQL Commands

Use `SYNTAX <command>` to get detailed syntax and examples for any command:

```sql
SYNTAX              -- list all commands
SYNTAX ATTACH       -- detailed syntax for ATTACH
SYNTAX IMPORT FUNCTION  -- detailed syntax for IMPORT FUNCTION
```

#### Available Commands

**Data**: ATTACH, DETACH, REPLACE, FILTER, IMPORT JOIN
**Schema**: JOIN, DROP JOIN, RENAME JOIN, ADD COLUMN, DROP COLUMN, RENAME COLUMN, CAST COLUMN, CREATE VIEW, DROP VIEW, RENAME VIEW
**Sources**: IMPORT [TEMP] FUNCTION, IMPORT [TEMP] CONNECTOR, DROP/RENAME [TEMP] FUNCTION/CONNECTOR, CREATE SOURCE, FETCH, FETCH ALL, DESCRIBE FUNCTION, DESCRIBE CONNECTOR
**Indexes**: CREATE INDEX, DROP INDEX, REBUILD INDEX, REINDEX
**Version Control**: COMMIT, RESET, UNDO, VERIFY DATA, EXPLAIN
**Metadata**: SET NAME, SET DESCRIPTION, SET CONFIG, SAVE CONFIG
**Export**: EXPORT TO '<path>' <sql> (formats: .csv, .jsonl)
**Introspection**: SHOW (DETAILS, HISTORY, STATUS, VIEWS, INDEXES, PACKS, BLOCKS, CONFIG, CONNECTORS, FUNCTIONS), SYNTAX

**Bundle references**: ATTACH and JOIN accept `bundle://` URLs to reference another bundle's query output:
`ATTACH 'bundle:///path/to/bundle'`, `JOIN 'bundle://./other' AS other ON ...`, `bundle+s3://bucket/path` for remote.

**Built-in SQL functions**: `search('<index_name>', '<query>')` for full-text search with BM25 scoring.

Standard SQL (SELECT, WITH, etc.) uses Apache DataFusion syntax. The table name is always `bundle`.
