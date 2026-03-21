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
# Non-interactive execute mode (agent-friendly, best for one-shot operations)
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

### MCP Usage

MCP mode keeps the bundle open across calls — use for multi-step workflows.

```bash
# Start as MCP server (configure in your AI assistant's MCP settings)
bundlebase --bundle ./my-bundle --mode mcp
bundlebase --bundle ./my-bundle --mode mcp --create
bundlebase --bundle ./my-bundle --mode mcp --read-only
```

MCP tools: `query` (any SQL/command), `schema`, `count`, `sample`, `status`, `history`.

Prefer CLI (`--execute`) for one-shot queries. Prefer MCP for building bundles or multi-step exploration.

### SQL Commands

Use `SYNTAX <command>` to get detailed syntax and examples for any command:

```sql
SYNTAX              -- list all commands
SYNTAX ATTACH       -- detailed syntax for ATTACH
SYNTAX IMPORT FUNCTION  -- detailed syntax for IMPORT FUNCTION
```

#### Available Commands

**Data**: ATTACH, DETACH, REPLACE, FILTER
**Schema**: JOIN, DROP JOIN, RENAME JOIN, ADD COLUMN, DROP COLUMN, RENAME COLUMN, CAST COLUMN, CREATE VIEW, DROP VIEW, RENAME VIEW
**Sources**: IMPORT [TEMP] FUNCTION, IMPORT [TEMP] CONNECTOR, DROP/RENAME [TEMP] FUNCTION/CONNECTOR, CREATE SOURCE, FETCH, FETCH ALL, DESCRIBE FUNCTION, DESCRIBE CONNECTOR
**Indexes**: CREATE INDEX, DROP INDEX, REBUILD INDEX, REINDEX
**Version Control**: COMMIT, RESET, UNDO, VERIFY DATA, EXPLAIN
**Metadata**: SET NAME, SET DESCRIPTION, SET CONFIG, SAVE CONFIG
**Introspection**: SHOW (DETAILS, HISTORY, STATUS, VIEWS, INDEXES, PACKS, BLOCKS, CONFIG, CONNECTORS, FUNCTIONS), SYNTAX

**Built-in SQL functions**: `search('<index_name>', '<query>')` for full-text search with BM25 scoring.

Standard SQL (SELECT, WITH, etc.) uses Apache DataFusion syntax. The table name is always `bundle`.
