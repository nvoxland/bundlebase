---
name: bundlebase
description: >
  Persistent, versioned, queryable data layer. Use when the user wants to
  analyze CSV/Parquet/JSON files, explore Kaggle datasets, clean or transform
  data, join multiple data sources, build reusable datasets, share data with
  a team, version data changes, create data pipelines, fetch data from APIs
  or cloud storage, or do any multi-step data work that should persist.
---

# Bundlebase

Bundlebase is a persistent, versioned, queryable data layer — Docker for data. It packages data files into bundles with a SQL query engine, custom functions, connectors to external sources (Kaggle, S3, APIs), and CLI/MCP/Python interfaces.

## When to Use Bundlebase

Use bundlebase when the data work should **persist, accumulate, or be shared** — not just run once in a script.

| Scenario | Use Bundlebase | Use pandas/polars directly |
|----------|---------------|--------------------------|
| One-off quick analysis of a small file | | X |
| Data that multiple people need to access | X | |
| Combining data from multiple sources (files, Kaggle, APIs) | X | |
| Iterative exploration (query, filter, query again) | X | |
| Data that needs versioning (undo, history, audit trail) | X | |
| Building a reusable, cleaned dataset | X | |
| Data pipeline that runs repeatedly | X | |
| Quick throwaway calculation | | X |

If a bundle already exists in the project (look for a `.bundlebase/` directory or a bundle path in project config), use bundlebase to work with it.

## Installation

```
pip install bundlebase
```

This installs the `bundlebase` CLI command.

## Choosing CLI vs MCP Mode

Bundlebase offers two agent-friendly modes. Choose based on the task:

**CLI mode** — Best for one-shot operations. `bundlebase query` for read-only queries, `bundlebase extend` for mutations (auto-commits after each command).

**MCP mode (`bundlebase mcp`)** — Best for building up a new bundle or other multi-step operations. The bundle stays open across calls, preserving cache and state. Use when you need to attach multiple files, run a sequence of transformations, explore data iteratively, or build up changes before committing.

| Scenario | Use |
|----------|-----|
| Check schema or row count | `query` |
| Run a single SELECT query | `query` |
| One-off ATTACH, FILTER, or other mutation | `extend` |
| Create a bundle from scratch with multiple files | MCP |
| Iterative data exploration (query, filter, query again) | MCP |
| Multi-step transformations (attach, filter, rename, commit) | MCP |
| Building up joins and views | MCP |

## CLI Commands

### `bundlebase query` — Read-only queries

Opens the bundle in read-only mode. Use for SELECT, EXPLAIN, SHOW, SYNTAX, and meta-commands.

```bash
# Pipe SQL from stdin (preferred — avoids shell quoting issues)
echo "SELECT * FROM bundle LIMIT 10" | bundlebase query --bundle ./my-bundle --format json
echo "SHOW COLUMNS" | bundlebase query --bundle ./my-bundle --format json
echo "SHOW COUNT" | bundlebase query --bundle ./my-bundle --format json

# Or pass SQL as an argument
bundlebase query --bundle ./my-bundle "SELECT * FROM bundle LIMIT 10" --format json
```

| Flag | Purpose |
|------|---------|
| `--bundle <path>` | Path to bundle (local path or `s3://...`) |
| `--format json` | JSON output (default: `table`) |
| `--config <path>` | YAML/JSON config file |

**Tip:** Piping SQL via stdin (`echo "..." | bundlebase query`) is simpler than passing it as an argument — no need to worry about escaping quotes for the shell.

### `bundlebase extend` — Mutating commands (auto-commits)

Opens the bundle in read-write mode. Executes the command(s) and **automatically commits** afterward. Use `-m` to provide a commit message; otherwise one is generated from the command. Multiple statements can be separated with `;` — all changes are committed together as a single commit.

```bash
# Pipe SQL from stdin (preferred — avoids shell quoting issues)
echo "ATTACH 'data.csv'" | bundlebase extend --bundle ./my-bundle --create
echo "RENAME COLUMN fname TO first_name" | bundlebase extend --bundle ./my-bundle -m "Cleaned up names"

# Multiple statements in one call (committed together)
echo "DROP COLUMN internal_id; RENAME COLUMN fname TO first_name" | bundlebase extend --bundle ./my-bundle -m "Initial cleanup"

# Or pass SQL as an argument
bundlebase extend --bundle ./my-bundle --create "ATTACH 'data.csv'"
```

| Flag | Purpose |
|------|---------|
| `--bundle <path>` | Path to bundle (local path or `s3://...`) |
| `--create` | Create new bundle if it doesn't exist |
| `-m, --message` | Commit message (auto-generated if omitted) |
| `--format json` | JSON output (default: `table`) |
| `--config <path>` | YAML/JSON config file |

`bundlebase execute` is an alias for `bundlebase extend`.

**Multiple statements:** Separate with `;`. All statements are validated before any execute — if one has a syntax error, none will run. Keep multi-statement calls short (2–3 statements). Longer chains are harder to debug when one fails partway through, and the time spent executing prior statements is wasted. For complex multi-step workflows, prefer MCP mode or separate `extend` calls.

JSON mode outputs arrays of objects for queries, single values/objects for commands. Errors go to stderr as `{"error": "..."}`. Exit code 1 on error.

Query results are hard-limited to 1000 rows. Use `LIMIT` in SQL for fewer.

## MCP Mode

MCP mode runs bundlebase as a Model Context Protocol server over stdio. Configure it as an MCP server in your AI assistant (Claude Code, Cursor, etc.) and the tools become available directly.

### MCP Server Configuration

Add to your MCP settings (e.g., Claude Code `mcp_servers` config):

```json
{
  "bundlebase": {
    "command": "bundlebase",
    "args": ["mcp", "--bundle", "./my-bundle"]
  }
}
```

Add `--create` to the args to create a new bundle, or `--read-only` for read-only access.

### Available MCP Tools

| Tool | Parameters | Description |
|------|------------|-------------|
| `query` | `sql` (string) | Execute any SQL query or bundlebase command. Returns JSON. 1000-row limit. |
| `schema` | (none) | Get column names, data types, and nullability |
| `count` | (none) | Get total row count |
| `sample` | `limit` (optional, default 10) | Preview sample rows as JSON |
| `status` | (none) | Show uncommitted changes |
| `history` | (none) | Show commit history |

The `query` tool handles everything: SELECT queries, ATTACH, DETACH, FILTER, RENAME, COMMIT, and all other bundlebase SQL commands.

### MCP Workflow Example

```
1. Call `schema` to understand the data structure
2. Call `sample` to preview the data
3. Call `query` with SQL to explore and transform
4. Call `status` to review uncommitted changes
5. Call `query` with "COMMIT 'message'" to save
```

## Common Workflows

### 1. Analyze a Data File

```bash
# Create bundle and load data
echo "ATTACH 'sales.csv'" | bundlebase extend --bundle ./analysis --create -m "Loaded sales data"

# Explore the schema
echo "SHOW COLUMNS" | bundlebase query --bundle ./analysis --format json

# Count rows
echo "SHOW COUNT" | bundlebase query --bundle ./analysis --format json

# Run queries
echo "SELECT department, COUNT(*) as cnt, AVG(salary) as avg_salary FROM bundle GROUP BY department ORDER BY avg_salary DESC" | bundlebase query --bundle ./analysis --format json
```

### 2. Clean and Transform Data

```bash
# Drop unnecessary columns and rename others (committed together)
echo "DROP COLUMN internal_id; DROP COLUMN debug_notes" | bundlebase extend --bundle ./clean -m "Removed internal columns"
echo "RENAME COLUMN fname TO first_name; RENAME COLUMN lname TO last_name" | bundlebase extend --bundle ./clean -m "Standardized names"

# Add a computed column
echo "ADD COLUMN full_name first_name || ' ' || last_name" | bundlebase extend --bundle ./clean

# Filter out bad data
echo "FILTER WITH SELECT * FROM bundle WHERE email IS NOT NULL" | bundlebase extend --bundle ./clean -m "Removed rows without email"
```

### 3. Join Multiple Data Sources

```bash
# Start with a base dataset
echo "ATTACH 'customers.parquet'" | bundlebase extend --bundle ./combined --create

# Join with orders
echo "JOIN 'orders.csv' AS orders ON id = orders.customer_id" | bundlebase extend --bundle ./combined

# Query across joined data
echo "SELECT c.name, COUNT(orders.id) as order_count, SUM(orders.amount) as total FROM bundle c JOIN orders ON c.id = orders.customer_id GROUP BY c.name ORDER BY total DESC LIMIT 10" | bundlebase query --bundle ./combined --format json

# Remove a join when no longer needed
echo "DROP JOIN orders" | bundlebase extend --bundle ./combined
```

### 4. Work with Multiple File Formats

```bash
# Attach multiple files in one call (committed together)
echo "ATTACH 'data.csv'; ATTACH 'more_data.parquet'; ATTACH 'extra.json'" | bundlebase extend --bundle ./multi --create -m "Loaded all data files"

# Replace a data source with updated version
echo "REPLACE 'data.csv' WITH 'data_v2.csv'" | bundlebase extend --bundle ./multi

# Detach a file
echo "DETACH 'extra.json'" | bundlebase extend --bundle ./multi
```

### 5. Create Views for Reusable Queries

```bash
# Create named views
echo "CREATE VIEW active_users AS SELECT * FROM bundle WHERE status = 'active'" | bundlebase extend --bundle ./reports
echo "CREATE VIEW high_value AS SELECT * FROM bundle WHERE lifetime_value > 10000" | bundlebase extend --bundle ./reports

# Query views like tables
echo "SELECT * FROM active_users LIMIT 5" | bundlebase query --bundle ./reports --format json

# Drop a view
echo "DROP VIEW high_value" | bundlebase extend --bundle ./reports
```

### 6. Full-Text Search

```bash
# Create a text index on a column
echo "CREATE TEXT INDEX ON description" | bundlebase extend --bundle ./docs

# Search with BM25 relevance scoring
echo "SELECT title, _score FROM search('description', 'machine learning') ORDER BY _score DESC LIMIT 10" | bundlebase query --bundle ./docs --format json

# Combine search with filters
echo "SELECT * FROM search('description', 'neural networks') WHERE category = 'AI'" | bundlebase query --bundle ./docs --format json
```

### 7. Version Control

```bash
# View history
echo "SHOW HISTORY" | bundlebase query --bundle ./data --format json

# View uncommitted changes
echo "SHOW STATUS" | bundlebase query --bundle ./data --format json

# Undo last commit
echo "UNDO" | bundlebase extend --bundle ./data

# Discard uncommitted changes
echo "RESET" | bundlebase extend --bundle ./data

# Verify data integrity
echo "VERIFY DATA" | bundlebase query --bundle ./data
```

### 8. Indexes for Performance

```bash
# Create a column index for faster filtering
echo "CREATE COLUMN INDEX ON customer_id" | bundlebase extend --bundle ./data

# Create a text index for full-text search
echo "CREATE TEXT INDEX ON description" | bundlebase extend --bundle ./data

# Rebuild a specific index
echo "REBUILD INDEX ON customer_id" | bundlebase extend --bundle ./data

# Rebuild all indexes
echo "REINDEX" | bundlebase extend --bundle ./data

# Drop an index
echo "DROP INDEX customer_id" | bundlebase extend --bundle ./data
```

### 9. Data Sources and Fetch

```bash
# Create a source pointing to a directory of files
echo "CREATE SOURCE my_connector WITH (url = 's3://bucket/data/')" | bundlebase extend --bundle ./pipeline

# Preview what fetch would do (dry run)
echo "FETCH base ADD DRY RUN" | bundlebase query --bundle ./pipeline --format json

# Actually fetch new files
echo "FETCH base ADD" | bundlebase extend --bundle ./pipeline

# Fetch all sources
echo "FETCH ALL SYNC" | bundlebase extend --bundle ./pipeline
```

### 10. Bundle Metadata

```bash
# Set bundle name and description (committed together)
echo "SET NAME 'Q4 Sales Report'; SET DESCRIPTION 'Quarterly sales data with regional breakdowns'" | bundlebase extend --bundle ./data

# Set runtime config
echo "SET CONFIG max_rows = '5000'" | bundlebase extend --bundle ./data

# Save config to bundle manifest
echo "SAVE CONFIG max_rows = '5000'" | bundlebase extend --bundle ./data
```

### 11. Query Execution Plans

```bash
# See how a query will execute
echo "EXPLAIN SELECT * FROM bundle WHERE salary > 50000" | bundlebase query --bundle ./data

# With execution statistics
echo "EXPLAIN ANALYZE SELECT * FROM bundle WHERE salary > 50000" | bundlebase query --bundle ./data

# Tree format
echo "EXPLAIN VERBOSE FORMAT TREE" | bundlebase query --bundle ./data
```

### 12. Remote Bundles

```bash
# Query a bundle from S3
echo "SELECT COUNT(*) FROM bundle" | bundlebase query --bundle s3://mybucket/my-bundle --format json

# Read-only schema check
echo "SHOW COLUMNS" | bundlebase query --bundle s3://mybucket/my-bundle --format json
```

## Fetching External Data with Connectors

Bundlebase has built-in connectors for common data sources. The pattern is: CREATE SOURCE → FETCH → query/transform.

**Built-in connectors:** `kaggle`, `remote_dir` (S3/GCS/Azure/local dirs), `ftp_directory`, `sftp_directory`, `web_scrape`, `postgres`

**Important:** Bundlebase's connectors call external APIs directly — you do **not** need to install separate CLI tools. For example, the `kaggle` connector uses the Kaggle REST API directly; there is no need to install the `kaggle` pip package or CLI. It only requires a `~/.kaggle/kaggle.json` credentials file (for public datasets, create one at kaggle.com → Settings → API → Create New Token).

```bash
# Kaggle: download a public dataset (no kaggle CLI needed — just ~/.kaggle/kaggle.json)
echo "CREATE SOURCE kaggle WITH (dataset = 'zillow/zecon', patterns = '*.csv')" | bundlebase extend --bundle ./housing --create
echo "FETCH base ADD" | bundlebase extend --bundle ./housing

# S3: attach all parquet files from a bucket
echo "CREATE SOURCE remote_dir WITH (url = 's3://my-bucket/data/', patterns = '**/*.parquet')" | bundlebase extend --bundle ./logs --create
echo "FETCH base ADD" | bundlebase extend --bundle ./logs

# Preview what would be fetched without actually fetching
echo "FETCH base ADD DRY RUN" | bundlebase query --bundle ./logs --format json

# Check what sources are configured
echo "SHOW CONNECTORS" | bundlebase query --bundle ./logs --format json
```

Use `SYNTAX CREATE SOURCE` and `SYNTAX FETCH` for detailed syntax. See the [Sources guide](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/sources.md) for full connector documentation.

## Building a Custom Connector

When data lives behind a custom API or needs custom fetch logic, write a Python connector:

```python
# my_connector.py
from bundlebase_sdk import Connector, Location, serve

class MyApiConnector(Connector):
    def discover(self, attached_locations, **kwargs):
        # Return available data locations (e.g., from an API listing endpoint)
        return [Location("users.parquet", format="parquet", version="v1")]

    def data(self, location, **kwargs):
        # Fetch and return data for a specific location
        import pyarrow as pa
        # ... call your API here ...
        return pa.table({"id": [1, 2], "name": ["Alice", "Bob"]})

if __name__ == "__main__":
    serve(MyApiConnector())
```

Register and use it:

```bash
# Install the SDK: pip install bundlebase-sdk
# Register the connector (temp = session-only, supports Python runtime)
echo "IMPORT TEMP CONNECTOR my.api FROM 'python::my_connector.py:MyApiConnector'" | bundlebase extend --bundle ./data
echo "CREATE SOURCE my.api" | bundlebase extend --bundle ./data
echo "FETCH base ADD" | bundlebase extend --bundle ./data
```

For persistent connectors (survive across sessions), use `ipc` or `ffi` runtimes instead of `python`. See the [Custom Connectors guide](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/index.md) and [Python SDK](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/python.md).

## Transforming Data with Functions and Computed Columns

After attaching data, use computed columns and custom functions to clean and enrich it:

```bash
# Add computed columns using SQL expressions
echo "ADD COLUMN full_name AS first_name || ' ' || last_name" | bundlebase extend --bundle ./data
echo "ADD COLUMN price_cents AS CAST(price * 100 AS INTEGER)" | bundlebase extend --bundle ./data

# Cast column types with optional regex cleanup (strip non-numeric chars before casting)
echo "CAST COLUMN price TO integer CLEAN '[^0-9]'" | bundlebase extend --bundle ./data

# Filter out bad rows
echo "FILTER WITH SELECT * FROM bundle WHERE email IS NOT NULL" | bundlebase extend --bundle ./data

# Use a custom Python function for complex transformations
# First, create the function file:
#   from bundlebase_sdk import Function
#   class NormalizePhone(Function):
#       def call(self, phone: str) -> str:
#           return re.sub(r'[^0-9+]', '', phone)
# Then register and use it:
echo "IMPORT TEMP FUNCTION util.normalize_phone FROM 'python::normalize.py:NormalizePhone'" | bundlebase extend --bundle ./data
echo "ADD COLUMN clean_phone AS util.normalize_phone(phone)" | bundlebase extend --bundle ./data -m "Cleaned and enriched data"
```

Use `SYNTAX ADD COLUMN`, `SYNTAX CAST COLUMN`, and `SYNTAX IMPORT FUNCTION` for detailed syntax. See the [Functions guide](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/functions.md).

## Using Bundlebase in Python Scripts

For data pipelines, notebooks, or automation scripts, use the Python API directly instead of CLI subprocess calls:

```python
import bundlebase.sync as bb

# Create or open a bundle
bundle = bb.create("./my-analysis")

# Attach data and transform
bundle.attach("sales.csv")
bundle.rename_column("fname", "first_name")
bundle.filter("revenue > 0")
bundle.add_column("total", "price * quantity")

# Query and export to pandas
df = bundle.query("SELECT region, SUM(total) as revenue FROM bundle GROUP BY region").to_pandas()

# Commit the cleaned version
bundle.commit("Cleaned sales data")
```

For async contexts (e.g., web servers), use `import bundlebase` with `await`. See the [Quick Start](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/getting-started/quick-start.md) for full API reference.

## Exporting Data

Use `EXPORT TO` to save query results directly to a file. This is more efficient than piping query output through stdout, especially for large result sets — use it instead of `SELECT` when you need the data in a file.

```bash
# Export to CSV
echo "EXPORT TO 'output.csv' SELECT * FROM bundle" | bundlebase query --bundle ./analysis

# Export filtered results to JSON Lines
echo "EXPORT TO 'active_users.jsonl' SELECT * FROM bundle WHERE active = true" | bundlebase query --bundle ./analysis

# Export aggregated results
echo "EXPORT TO 'summary.csv' SELECT department, COUNT(*) as cnt, AVG(salary) as avg_sal FROM bundle GROUP BY department" | bundlebase query --bundle ./analysis
```

**Supported formats:** `.csv`, `.jsonl` (JSON Lines — one JSON object per line)

**Tip:** For data exploration where you need to see results, use `SELECT` with `--format json`. For saving data to a file for further processing, prefer `EXPORT TO` — it streams directly to the file without row limits.

### Sharing Bundles

Bundles are portable — share them with teammates:

```bash
# Push a bundle to S3 so others can access it
echo "ATTACH 'cleaned.parquet'" | bundlebase extend --bundle s3://team-bucket/shared-analysis --create -m "Shared cleaned dataset"

# Others can then query it
echo "SHOW COLUMNS" | bundlebase query --bundle s3://team-bucket/shared-analysis --format json
```

## SQL Reference Summary

The table name for bundle data is always `bundle`. Standard SQL (Apache DataFusion syntax) is supported for SELECT queries.

Use `SYNTAX` to get command syntax on demand:

```bash
# List all available commands
echo "SYNTAX" | bundlebase query --bundle ./data

# Get detailed syntax and examples for a specific command
echo "SYNTAX IMPORT FUNCTION" | bundlebase query --bundle ./data
echo "SYNTAX ATTACH" | bundlebase query --bundle ./data
```

In MCP mode, use the `query` tool with `SYNTAX <command>`.

For a quick command reference, see [reference.md](reference.md).

### REPL Meta-Commands

When using the interactive REPL (`bundlebase repl`), these meta-commands are available:

| Command | Purpose |
|---------|---------|
| `/help` | Show available commands |
| `/clear` | Clear the terminal |
| `/exit` | Exit the REPL |

For inspecting bundle data and metadata, use SQL commands directly:

| SQL Command | Purpose |
|-------------|---------|
| `SELECT * FROM bundle` | Display all data |
| `SHOW COLUMNS` | Show bundle schema |
| `SHOW COUNT` | Count rows |
| `SHOW STATUS` | Show uncommitted changes |
| `SHOW HISTORY` | Show version history |
| `SHOW DETAILS` | Show bundle metadata |

These SQL commands also work with `bundlebase query`, e.g. `echo "SHOW COLUMNS" | bundlebase query --bundle ./data --format json`.
