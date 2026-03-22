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
# Query the bundle
bundlebase query --bundle ./my-bundle "SELECT * FROM bundle LIMIT 10" --format json

# Explore schema and row count
bundlebase query --bundle ./my-bundle "SHOW COLUMNS" --format json
bundlebase query --bundle ./my-bundle "SHOW COUNT" --format json

# Pipe SQL from stdin
echo "SELECT count(*) FROM bundle" | bundlebase query --bundle ./my-bundle --format json
```

| Flag | Purpose |
|------|---------|
| `--bundle <path>` | Path to bundle (local path or `s3://...`) |
| `--format json` | JSON output (default: `table`) |
| `--config <path>` | YAML/JSON config file |

### `bundlebase extend` — Mutating commands (auto-commits)

Opens the bundle in read-write mode. Executes the command and **automatically commits** afterward. Use `-m` to provide a commit message; otherwise one is generated from the command.

```bash
# Create a new bundle and attach data (auto-commits)
bundlebase extend --bundle ./my-bundle --create "ATTACH 'data.csv'"

# Mutate with a custom commit message
bundlebase extend --bundle ./my-bundle -m "Cleaned up names" "RENAME COLUMN fname TO first_name"

# Pipe SQL from stdin
echo "FILTER WITH SELECT * FROM bundle WHERE active" | bundlebase extend --bundle ./my-bundle
```

| Flag | Purpose |
|------|---------|
| `--bundle <path>` | Path to bundle (local path or `s3://...`) |
| `--create` | Create new bundle if it doesn't exist |
| `-m, --message` | Commit message (auto-generated if omitted) |
| `--format json` | JSON output (default: `table`) |
| `--config <path>` | YAML/JSON config file |

`bundlebase execute` is an alias for `bundlebase extend`.

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
bundlebase extend --bundle ./analysis --create "ATTACH 'sales.csv'" -m "Loaded sales data"

# Explore the schema
bundlebase query --bundle ./analysis "SHOW COLUMNS" --format json

# Count rows
bundlebase query --bundle ./analysis "SHOW COUNT" --format json

# Run queries
bundlebase query --bundle ./analysis "SELECT department, COUNT(*) as cnt, AVG(salary) as avg_salary FROM bundle GROUP BY department ORDER BY avg_salary DESC" --format json
```

### 2. Clean and Transform Data

```bash
# Drop unnecessary columns
bundlebase extend --bundle ./clean "DROP COLUMN internal_id"
bundlebase extend --bundle ./clean "DROP COLUMN debug_notes"

# Rename columns for clarity
bundlebase extend --bundle ./clean "RENAME COLUMN fname TO first_name"
bundlebase extend --bundle ./clean "RENAME COLUMN lname TO last_name"

# Add a computed column
bundlebase extend --bundle ./clean "ADD COLUMN full_name first_name || ' ' || last_name"

# Filter out bad data
bundlebase extend --bundle ./clean "FILTER WITH SELECT * FROM bundle WHERE email IS NOT NULL" -m "Cleaned and standardized columns"
```

### 3. Join Multiple Data Sources

```bash
# Start with a base dataset
bundlebase extend --bundle ./combined --create "ATTACH 'customers.parquet'"

# Join with orders
bundlebase extend --bundle ./combined "JOIN 'orders.csv' AS orders ON id = orders.customer_id"

# Query across joined data
bundlebase query --bundle ./combined "SELECT c.name, COUNT(orders.id) as order_count, SUM(orders.amount) as total FROM bundle c JOIN orders ON c.id = orders.customer_id GROUP BY c.name ORDER BY total DESC LIMIT 10" --format json

# Remove a join when no longer needed
bundlebase extend --bundle ./combined "DROP JOIN orders"
```

### 4. Work with Multiple File Formats

```bash
# Attach CSV, Parquet, and JSON files to the same bundle
bundlebase extend --bundle ./multi --create "ATTACH 'data.csv'"
bundlebase extend --bundle ./multi "ATTACH 'more_data.parquet'"
bundlebase extend --bundle ./multi "ATTACH 'extra.json'"

# Replace a data source with updated version
bundlebase extend --bundle ./multi "REPLACE 'data.csv' WITH 'data_v2.csv'"

# Detach a file
bundlebase extend --bundle ./multi "DETACH 'extra.json'"
```

### 5. Create Views for Reusable Queries

```bash
# Create named views
bundlebase extend --bundle ./reports "CREATE VIEW active_users AS SELECT * FROM bundle WHERE status = 'active'"
bundlebase extend --bundle ./reports "CREATE VIEW high_value AS SELECT * FROM bundle WHERE lifetime_value > 10000"

# Query views like tables
bundlebase query --bundle ./reports "SELECT * FROM active_users LIMIT 5" --format json

# Drop a view
bundlebase extend --bundle ./reports "DROP VIEW high_value"
```

### 6. Full-Text Search

```bash
# Create a text index on a column
bundlebase extend --bundle ./docs "CREATE TEXT INDEX ON description"

# Search with BM25 relevance scoring
bundlebase query --bundle ./docs "SELECT title, _score FROM search('description', 'machine learning') ORDER BY _score DESC LIMIT 10" --format json

# Combine search with filters
bundlebase query --bundle ./docs "SELECT * FROM search('description', 'neural networks') WHERE category = 'AI'" --format json
```

### 7. Version Control

```bash
# View history
bundlebase query --bundle ./data "SHOW HISTORY" --format json

# View uncommitted changes
bundlebase query --bundle ./data "SHOW STATUS" --format json

# Undo last commit
bundlebase extend --bundle ./data "UNDO"

# Discard uncommitted changes
bundlebase extend --bundle ./data "RESET"

# Verify data integrity
bundlebase query --bundle ./data "VERIFY DATA"
```

### 8. Indexes for Performance

```bash
# Create a column index for faster filtering
bundlebase extend --bundle ./data "CREATE COLUMN INDEX ON customer_id"

# Create a text index for full-text search
bundlebase extend --bundle ./data "CREATE TEXT INDEX ON description"

# Rebuild a specific index
bundlebase extend --bundle ./data "REBUILD INDEX ON customer_id"

# Rebuild all indexes
bundlebase extend --bundle ./data "REINDEX"

# Drop an index
bundlebase extend --bundle ./data "DROP INDEX customer_id"
```

### 9. Data Sources and Fetch

```bash
# Create a source pointing to a directory of files
bundlebase extend --bundle ./pipeline "CREATE SOURCE my_connector WITH (url = 's3://bucket/data/')"

# Preview what fetch would do (dry run)
bundlebase query --bundle ./pipeline "FETCH base ADD DRY RUN" --format json

# Actually fetch new files
bundlebase extend --bundle ./pipeline "FETCH base ADD"

# Fetch all sources
bundlebase extend --bundle ./pipeline "FETCH ALL SYNC"
```

### 10. Bundle Metadata

```bash
# Set bundle name and description
bundlebase extend --bundle ./data "SET NAME 'Q4 Sales Report'"
bundlebase extend --bundle ./data "SET DESCRIPTION 'Quarterly sales data with regional breakdowns'"

# Set runtime config
bundlebase extend --bundle ./data "SET CONFIG max_rows = '5000'"

# Save config to bundle manifest
bundlebase extend --bundle ./data "SAVE CONFIG max_rows = '5000'"
```

### 11. Query Execution Plans

```bash
# See how a query will execute
bundlebase query --bundle ./data "EXPLAIN SELECT * FROM bundle WHERE salary > 50000"

# With execution statistics
bundlebase query --bundle ./data "EXPLAIN ANALYZE SELECT * FROM bundle WHERE salary > 50000"

# Tree format
bundlebase query --bundle ./data "EXPLAIN VERBOSE FORMAT TREE"
```

### 12. Remote Bundles

```bash
# Query a bundle from S3
bundlebase query --bundle s3://mybucket/my-bundle "SELECT COUNT(*) FROM bundle" --format json

# Read-only schema check
bundlebase query --bundle s3://mybucket/my-bundle "SHOW COLUMNS" --format json
```

## Fetching External Data with Connectors

Bundlebase has built-in connectors for common data sources. The pattern is: CREATE SOURCE → FETCH → query/transform.

**Built-in connectors:** `kaggle`, `remote_dir` (S3/GCS/Azure/local dirs), `ftp_directory`, `sftp_directory`, `web_scrape`, `postgres`

**Important:** Bundlebase's connectors call external APIs directly — you do **not** need to install separate CLI tools. For example, the `kaggle` connector uses the Kaggle REST API directly; there is no need to install the `kaggle` pip package or CLI. It only requires a `~/.kaggle/kaggle.json` credentials file (for public datasets, create one at kaggle.com → Settings → API → Create New Token).

```bash
# Kaggle: download a public dataset (no kaggle CLI needed — just ~/.kaggle/kaggle.json)
bundlebase extend --bundle ./housing --create "CREATE SOURCE kaggle WITH (dataset = 'zillow/zecon', patterns = '*.csv')"
bundlebase extend --bundle ./housing "FETCH base ADD"

# S3: attach all parquet files from a bucket
bundlebase extend --bundle ./logs --create "CREATE SOURCE remote_dir WITH (url = 's3://my-bucket/data/', patterns = '**/*.parquet')"
bundlebase extend --bundle ./logs "FETCH base ADD"

# Preview what would be fetched without actually fetching
bundlebase query --bundle ./logs "FETCH base ADD DRY RUN" --format json

# Check what sources are configured
bundlebase query --bundle ./logs "SHOW CONNECTORS" --format json
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
bundlebase extend --bundle ./data "IMPORT TEMP CONNECTOR my.api FROM 'python::my_connector.py:MyApiConnector'"
bundlebase extend --bundle ./data "CREATE SOURCE my.api"
bundlebase extend --bundle ./data "FETCH base ADD"
```

For persistent connectors (survive across sessions), use `ipc` or `ffi` runtimes instead of `python`. See the [Custom Connectors guide](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/index.md) and [Python SDK](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/python.md).

## Transforming Data with Functions and Computed Columns

After attaching data, use computed columns and custom functions to clean and enrich it:

```bash
# Add computed columns using SQL expressions
bundlebase extend --bundle ./data "ADD COLUMN full_name AS first_name || ' ' || last_name"
bundlebase extend --bundle ./data "ADD COLUMN price_cents AS CAST(price * 100 AS INTEGER)"

# Cast column types with optional regex cleanup (strip non-numeric chars before casting)
bundlebase extend --bundle ./data "CAST COLUMN price TO integer CLEAN '[^0-9]'"

# Filter out bad rows
bundlebase extend --bundle ./data "FILTER WITH SELECT * FROM bundle WHERE email IS NOT NULL"

# Use a custom Python function for complex transformations
# First, create the function file:
#   from bundlebase_sdk import Function
#   class NormalizePhone(Function):
#       def call(self, phone: str) -> str:
#           return re.sub(r'[^0-9+]', '', phone)
# Then register and use it:
bundlebase extend --bundle ./data "IMPORT TEMP FUNCTION util.normalize_phone FROM 'python::normalize.py:NormalizePhone'"
bundlebase extend --bundle ./data "ADD COLUMN clean_phone AS util.normalize_phone(phone)" -m "Cleaned and enriched data"
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

## Sharing and Exporting Data

Bundles are portable — share them with teammates or export results for non-bundlebase users:

```bash
# Export query results as JSON
bundlebase query --bundle ./analysis "SELECT * FROM bundle" --format json > results.json

# Push a bundle to S3 so others can access it
bundlebase extend --bundle s3://team-bucket/shared-analysis --create "ATTACH 'cleaned.parquet'" -m "Shared cleaned dataset"

# Others can then query it
bundlebase query --bundle s3://team-bucket/shared-analysis "SHOW COLUMNS" --format json
```

For sharing with non-bundlebase users, export query results to standard formats (JSON, CSV) using `--format json` or by querying into a Python script and saving with pandas.

## SQL Reference Summary

The table name for bundle data is always `bundle`. Standard SQL (Apache DataFusion syntax) is supported for SELECT queries.

Use `SYNTAX` to get command syntax on demand:

```bash
# List all available commands
bundlebase query --bundle ./data "SYNTAX"

# Get detailed syntax and examples for a specific command
bundlebase query --bundle ./data "SYNTAX IMPORT FUNCTION"
bundlebase query --bundle ./data "SYNTAX ATTACH"
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

These SQL commands also work with `bundlebase query`, e.g. `bundlebase query --bundle ./data "SHOW COLUMNS" --format json`.
