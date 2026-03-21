---
name: bundlebase
description: >
  Work with bundlebase data bundles — Docker for data. Use when the user asks
  to analyze data files (CSV, Parquet, JSON), create data bundles, query data
  with SQL, transform or clean data, join multiple data sources, create custom
  functions or connectors, or version data changes.
---

# Bundlebase

Bundlebase packages data files into versioned bundles with a SQL query engine, custom functions, custom connectors, and a CLI interface. Think of it as Docker for data.

## Installation

```
pip install bundlebase
```

This installs the `bundlebase` CLI command.

## Choosing CLI vs MCP Mode

Bundlebase offers two agent-friendly modes. Choose based on the task:

**CLI mode (`--execute`)** — Best for one-shot simple queries or changes. Each invocation opens the bundle, runs a command, and exits. Use when you need a quick lookup, a single query, or a standalone mutation.

**MCP mode (`--mode mcp`)** — Best for building up a new bundle or other multi-step operations. The bundle stays open across calls, preserving cache and state. Use when you need to attach multiple files, run a sequence of transformations, explore data iteratively, or build up changes before committing.

| Scenario | Use |
|----------|-----|
| Check schema or row count | CLI |
| Run a single SELECT query | CLI |
| One-off ATTACH or COMMIT | CLI |
| Create a bundle from scratch with multiple files | MCP |
| Iterative data exploration (query, filter, query again) | MCP |
| Multi-step transformations (attach, filter, rename, commit) | MCP |
| Building up joins and views | MCP |

## CLI Mode

All operations use `--execute` for non-interactive, single-command execution. Add `--format json` for machine-readable output.

```bash
# Create a new bundle and attach data
bundlebase --bundle ./my-bundle --create --execute "ATTACH 'data.csv'"

# Query the bundle
bundlebase --bundle ./my-bundle --execute "SELECT * FROM bundle LIMIT 10" --format json

# Commit changes (version control)
bundlebase --bundle ./my-bundle --execute "COMMIT 'Initial data load'"
```

### Key CLI Flags

| Flag | Purpose |
|------|---------|
| `--bundle <path>` | Path to bundle (local path or `s3://...`) |
| `--create` | Create new bundle if it doesn't exist |
| `--execute "<sql>"` | Run one command and exit |
| `--format json` | JSON output (default: `table`) |
| `--read-only` | Open in read-only mode |
| `--config <path>` | YAML/JSON config file |

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
    "args": ["--bundle", "./my-bundle", "--mode", "mcp"]
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
bundlebase --bundle ./analysis --create --execute "ATTACH 'sales.csv'"

# Explore the schema
bundlebase --bundle ./analysis --execute "/schema" --format json

# Count rows
bundlebase --bundle ./analysis --execute "/count" --format json

# Run queries
bundlebase --bundle ./analysis --execute "SELECT department, COUNT(*) as cnt, AVG(salary) as avg_salary FROM bundle GROUP BY department ORDER BY avg_salary DESC" --format json

# Save your work
bundlebase --bundle ./analysis --execute "COMMIT 'Loaded sales data'"
```

### 2. Clean and Transform Data

```bash
# Drop unnecessary columns
bundlebase --bundle ./clean --execute "DROP COLUMN internal_id"
bundlebase --bundle ./clean --execute "DROP COLUMN debug_notes"

# Rename columns for clarity
bundlebase --bundle ./clean --execute "RENAME COLUMN fname TO first_name"
bundlebase --bundle ./clean --execute "RENAME COLUMN lname TO last_name"

# Add a computed column
bundlebase --bundle ./clean --execute "ADD COLUMN full_name first_name || ' ' || last_name"

# Filter out bad data
bundlebase --bundle ./clean --execute "FILTER WITH SELECT * FROM bundle WHERE email IS NOT NULL"

# Commit the cleaned version
bundlebase --bundle ./clean --execute "COMMIT 'Cleaned and standardized columns'"
```

### 3. Join Multiple Data Sources

```bash
# Start with a base dataset
bundlebase --bundle ./combined --create --execute "ATTACH 'customers.parquet'"

# Join with orders
bundlebase --bundle ./combined --execute "JOIN 'orders.csv' AS orders ON id = orders.customer_id"

# Query across joined data
bundlebase --bundle ./combined --execute "SELECT c.name, COUNT(orders.id) as order_count, SUM(orders.amount) as total FROM bundle c JOIN orders ON c.id = orders.customer_id GROUP BY c.name ORDER BY total DESC LIMIT 10" --format json

# Remove a join when no longer needed
bundlebase --bundle ./combined --execute "DROP JOIN orders"
```

### 4. Work with Multiple File Formats

```bash
# Attach CSV, Parquet, and JSON files to the same bundle
bundlebase --bundle ./multi --create --execute "ATTACH 'data.csv'"
bundlebase --bundle ./multi --execute "ATTACH 'more_data.parquet'"
bundlebase --bundle ./multi --execute "ATTACH 'extra.json'"

# Replace a data source with updated version
bundlebase --bundle ./multi --execute "REPLACE 'data.csv' WITH 'data_v2.csv'"

# Detach a file
bundlebase --bundle ./multi --execute "DETACH 'extra.json'"
```

### 5. Create Views for Reusable Queries

```bash
# Create named views
bundlebase --bundle ./reports --execute "CREATE VIEW active_users AS SELECT * FROM bundle WHERE status = 'active'"
bundlebase --bundle ./reports --execute "CREATE VIEW high_value AS SELECT * FROM bundle WHERE lifetime_value > 10000"

# Query views like tables
bundlebase --bundle ./reports --execute "SELECT * FROM active_users LIMIT 5" --format json

# Drop a view
bundlebase --bundle ./reports --execute "DROP VIEW high_value"
```

### 6. Full-Text Search

```bash
# Create a text index on a column
bundlebase --bundle ./docs --execute "CREATE TEXT INDEX ON description"

# Search with BM25 relevance scoring
bundlebase --bundle ./docs --execute "SELECT title, _score FROM search('description', 'machine learning') ORDER BY _score DESC LIMIT 10" --format json

# Combine search with filters
bundlebase --bundle ./docs --execute "SELECT * FROM search('description', 'neural networks') WHERE category = 'AI'" --format json
```

### 7. Version Control

```bash
# Commit changes
bundlebase --bundle ./data --execute "COMMIT 'Added Q4 sales data'"

# View history
bundlebase --bundle ./data --execute "/history" --format json

# View uncommitted changes
bundlebase --bundle ./data --execute "/status" --format json

# Undo last commit
bundlebase --bundle ./data --execute "UNDO"

# Discard uncommitted changes
bundlebase --bundle ./data --execute "RESET"

# Verify data integrity
bundlebase --bundle ./data --execute "VERIFY DATA"
```

### 8. Indexes for Performance

```bash
# Create a column index for faster filtering
bundlebase --bundle ./data --execute "CREATE COLUMN INDEX ON customer_id"

# Create a text index for full-text search
bundlebase --bundle ./data --execute "CREATE TEXT INDEX ON description"

# Rebuild a specific index
bundlebase --bundle ./data --execute "REBUILD INDEX ON customer_id"

# Rebuild all indexes
bundlebase --bundle ./data --execute "REINDEX"

# Drop an index
bundlebase --bundle ./data --execute "DROP INDEX customer_id"
```

### 9. Data Sources and Fetch

```bash
# Create a source pointing to a directory of files
bundlebase --bundle ./pipeline --execute "CREATE SOURCE my_connector WITH (url = 's3://bucket/data/')"

# Preview what fetch would do (dry run)
bundlebase --bundle ./pipeline --execute "FETCH base ADD DRY RUN" --format json

# Actually fetch new files
bundlebase --bundle ./pipeline --execute "FETCH base ADD"

# Fetch all sources
bundlebase --bundle ./pipeline --execute "FETCH ALL SYNC"
```

### 10. Bundle Metadata

```bash
# Set bundle name and description
bundlebase --bundle ./data --execute "SET NAME 'Q4 Sales Report'"
bundlebase --bundle ./data --execute "SET DESCRIPTION 'Quarterly sales data with regional breakdowns'"

# Set runtime config
bundlebase --bundle ./data --execute "SET CONFIG max_rows = '5000'"

# Save config to bundle manifest
bundlebase --bundle ./data --execute "SAVE CONFIG max_rows = '5000'"
```

### 11. Query Execution Plans

```bash
# See how a query will execute
bundlebase --bundle ./data --execute "EXPLAIN SELECT * FROM bundle WHERE salary > 50000"

# With execution statistics
bundlebase --bundle ./data --execute "EXPLAIN ANALYZE SELECT * FROM bundle WHERE salary > 50000"

# Tree format
bundlebase --bundle ./data --execute "EXPLAIN VERBOSE FORMAT TREE"
```

### 12. Remote Bundles

```bash
# Open a bundle from S3
bundlebase --bundle s3://mybucket/my-bundle --execute "SELECT COUNT(*) FROM bundle" --format json

# Read-only access
bundlebase --bundle s3://mybucket/my-bundle --read-only --execute "/schema" --format json
```

## Fetching External Data with Connectors

Bundlebase has built-in connectors for common data sources. The pattern is: CREATE SOURCE → FETCH → query/transform → COMMIT.

**Built-in connectors:** `kaggle`, `remote_dir` (S3/GCS/Azure/local dirs), `ftp_directory`, `sftp_directory`, `web_scrape`, `postgres`

```bash
# Kaggle: download a dataset (requires ~/.kaggle/kaggle.json credentials)
bundlebase --bundle ./housing --create --execute "CREATE SOURCE kaggle WITH (dataset = 'zillow/zecon', patterns = '*.csv')"
bundlebase --bundle ./housing --execute "FETCH base ADD"

# S3: attach all parquet files from a bucket
bundlebase --bundle ./logs --create --execute "CREATE SOURCE remote_dir WITH (url = 's3://my-bucket/data/', patterns = '**/*.parquet')"
bundlebase --bundle ./logs --execute "FETCH base ADD"

# Preview what would be fetched without actually fetching
bundlebase --bundle ./logs --execute "FETCH base ADD DRY RUN" --format json

# Check what sources are configured
bundlebase --bundle ./logs --execute "SHOW CONNECTORS" --format json
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
bundlebase --bundle ./data --execute "IMPORT TEMP CONNECTOR my.api FROM 'python::my_connector.py:MyApiConnector'"
bundlebase --bundle ./data --execute "CREATE SOURCE my.api"
bundlebase --bundle ./data --execute "FETCH base ADD"
```

For persistent connectors (survive across sessions), use `ipc` or `ffi` runtimes instead of `python`. See the [Custom Connectors guide](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/index.md) and [Python SDK](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/python.md).

## Transforming Data with Functions and Computed Columns

After attaching data, use computed columns and custom functions to clean and enrich it:

```bash
# Add computed columns using SQL expressions
bundlebase --bundle ./data --execute "ADD COLUMN full_name AS first_name || ' ' || last_name"
bundlebase --bundle ./data --execute "ADD COLUMN price_cents AS CAST(price * 100 AS INTEGER)"

# Cast column types with optional regex cleanup (strip non-numeric chars before casting)
bundlebase --bundle ./data --execute "CAST COLUMN price TO integer CLEAN '[^0-9]'"

# Filter out bad rows
bundlebase --bundle ./data --execute "FILTER WITH SELECT * FROM bundle WHERE email IS NOT NULL"

# Use a custom Python function for complex transformations
# First, create the function file:
#   from bundlebase_sdk import Function
#   class NormalizePhone(Function):
#       def call(self, phone: str) -> str:
#           return re.sub(r'[^0-9+]', '', phone)
# Then register and use it:
bundlebase --bundle ./data --execute "IMPORT TEMP FUNCTION util.normalize_phone FROM 'python::normalize.py:NormalizePhone'"
bundlebase --bundle ./data --execute "ADD COLUMN clean_phone AS util.normalize_phone(phone)"

# Commit the cleaned version
bundlebase --bundle ./data --execute "COMMIT 'Cleaned and enriched data'"
```

Use `SYNTAX ADD COLUMN`, `SYNTAX CAST COLUMN`, and `SYNTAX IMPORT FUNCTION` for detailed syntax. See the [Functions guide](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/functions.md).

## SQL Reference Summary

The table name for bundle data is always `bundle`. Standard SQL (Apache DataFusion syntax) is supported for SELECT queries.

Use `SYNTAX` to get command syntax on demand:

```bash
# List all available commands
bundlebase --bundle ./data --execute "SYNTAX"

# Get detailed syntax and examples for a specific command
bundlebase --bundle ./data --execute "SYNTAX IMPORT FUNCTION"
bundlebase --bundle ./data --execute "SYNTAX ATTACH"
```

In MCP mode, use the `query` tool with `SYNTAX <command>`.

For a quick command reference, see [reference.md](reference.md).

### REPL Meta-Commands

When using the interactive REPL (without `--execute`), these meta-commands are available:

| Command | Purpose |
|---------|---------|
| `/help` | Show available commands |
| `/show` | Display all data |
| `/schema` | Show bundle schema |
| `/count` | Count rows |
| `/status` | Show uncommitted changes |
| `/history` | Show version history |
| `/exit` | Exit the REPL |

These also work with `--execute`, e.g. `--execute "/schema" --format json`.
