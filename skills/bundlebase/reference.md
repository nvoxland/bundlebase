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
# Read-only queries
bundlebase query --bundle ./sales "SELECT * FROM bundle LIMIT 5" --format json
bundlebase query --bundle ./sales "/schema" --format json
bundlebase query --bundle ./sales "/count" --format json
echo "SELECT count(*) FROM bundle" | bundlebase query --bundle ./sales --format json

# Mutating commands (auto-commits after each)
bundlebase extend --bundle ./sales --create "ATTACH 'raw_sales.csv'"
bundlebase extend --bundle ./sales "FILTER WITH SELECT * FROM bundle WHERE amount > 0"
bundlebase extend --bundle ./sales -m "Cleaned sales data" "RENAME COLUMN amt TO amount"

# Interactive REPL
bundlebase repl --bundle ./my-bundle --create
bundlebase repl --bundle ./my-bundle
bundlebase repl --bundle ./my-bundle --read-only

# Remote bundles
bundlebase query --bundle s3://mybucket/my-bundle "SELECT * FROM bundle LIMIT 5" --format json
```

`--format`: `table` (default) or `json`. JSON mode outputs arrays of objects for queries, single values/objects for commands.
`bundlebase execute` is an alias for `bundlebase extend`.
`bundlebase extend` auto-commits after each command. Use `-m` for a custom commit message.
Query results are hard-limited to 1000 rows. Use `LIMIT` in SQL for fewer.

REPL meta-commands: `/help`, `/show`, `/schema`, `/count`, `/status`, `/history`, `/exit`

### MCP Usage

MCP mode keeps the bundle open across calls — use for multi-step workflows.

```bash
# Start as MCP server (configure in your AI assistant's MCP settings)
bundlebase mcp --bundle ./my-bundle
bundlebase mcp --bundle ./my-bundle --create
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

**Data**: ATTACH, DETACH, REPLACE, FILTER
**Schema**: JOIN, DROP JOIN, RENAME JOIN, ADD COLUMN, DROP COLUMN, RENAME COLUMN, CAST COLUMN, CREATE VIEW, DROP VIEW, RENAME VIEW
**Sources**: IMPORT [TEMP] FUNCTION, IMPORT [TEMP] CONNECTOR, DROP/RENAME [TEMP] FUNCTION/CONNECTOR, CREATE SOURCE, FETCH, FETCH ALL, DESCRIBE FUNCTION, DESCRIBE CONNECTOR
**Indexes**: CREATE INDEX, DROP INDEX, REBUILD INDEX, REINDEX
**Version Control**: COMMIT, RESET, UNDO, VERIFY DATA, EXPLAIN
**Metadata**: SET NAME, SET DESCRIPTION, SET CONFIG, SAVE CONFIG
**Introspection**: SHOW (DETAILS, HISTORY, STATUS, VIEWS, INDEXES, PACKS, BLOCKS, CONFIG, CONNECTORS, FUNCTIONS), SYNTAX

**Built-in SQL functions**: `search('<index_name>', '<query>')` for full-text search with BM25 scoring.

Standard SQL (SELECT, WITH, etc.) uses Apache DataFusion syntax. The table name is always `bundle`.
