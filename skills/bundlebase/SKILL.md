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

Bundlebase offers two agent-friendly modes:

**MCP mode (`bundlebase mcp`)** — **Always prefer MCP when the server is configured.** It keeps bundles open across calls, preserving cache and state, supports multiple bundles simultaneously, and gives better performance and feedback for any workflow.

**CLI mode** — Only use for true one-off operations where you don't need to interact with the bundle again:

- `bundlebase list-bundles` — discover bundles in a directory
- `bundlebase query` — a single read-only query
- `bundlebase create` — create a new bundle with initial data
- `bundlebase extend` — a single mutation (auto-commits after each call)

**Important:** Do NOT use MCP and CLI on the same bundle simultaneously — close the MCP bundle first.

## CLI Commands

### `bundlebase list-bundles` — Discover bundles

Scans a directory for bundles and shows their name and description.

```bash
# List bundles in the current directory
bundlebase list-bundles

# List bundles in a specific directory
bundlebase list-bundles --path /data/bundles

# List bundles in an S3 bucket
bundlebase list-bundles --path s3://my-bucket/bundles/
```

| Flag | Purpose |
|------|---------|
| `--path <path>` | Path or URL to search (default: `.`) |

**Always run `bundlebase list-bundles` first** when starting work in a directory that may already contain bundles, so you know what data is available before creating new bundles.

### `bundlebase query` — Read-only queries

Opens the bundle in read-only mode. Use for SELECT, EXPLAIN, SHOW, SYNTAX, and meta-commands.

```bash
bundlebase query --bundle ./my-bundle --format json "SELECT * FROM bundle LIMIT 10"
bundlebase query --bundle ./my-bundle --format json "SHOW COLUMNS"
bundlebase query --bundle ./my-bundle --format json "SHOW COUNT"
```

| Flag | Purpose |
|------|---------|
| `--bundle <path>` | Path to bundle (local path or `s3://...`) |
| `--format json` | JSON output (default: `table`) |
| `--config <path>` | YAML/JSON config file |

**Passing SQL:** Pass SQL as a command argument (preferred — avoids permission prompts from stdin piping):

```bash
bundlebase query --bundle ./data "SELECT * FROM bundle LIMIT 10"
bundlebase query --bundle ./data --format json "SHOW COLUMNS"
```

SQL uses single quotes for strings. In shell, wrap the whole SQL in double quotes: `bundlebase extend --bundle ./data "COMMIT 'my message'"`. To escape a single quote in SQL, double it: `"SELECT * FROM bundle WHERE name = 'O''Brien'"`.

### `bundlebase create` — Create a new bundle

Creates a new bundle at the specified path. Optionally executes initial commands (like ATTACH) and auto-commits.

**IMPORTANT: Always set a name and a helpful description when creating a bundle.** This makes bundles discoverable and understandable to other users and agents via `bundlebase list-bundles`. Include `SET NAME` and `SET DESCRIPTION` in the create command:

```bash
# Create and load initial data with name and description
bundlebase create --bundle ./my-bundle "SET NAME 'Sales Data'; SET DESCRIPTION 'Monthly sales records from the CRM export, filtered to US region'; ATTACH 'sales.csv'"

# Create with a custom commit message
bundlebase create --bundle ./analysis -m "Loaded sales data" "SET NAME 'Sales Analysis'; SET DESCRIPTION 'Q4 2025 sales analysis with regional breakdowns'; ATTACH 'sales.csv'"
```

| Flag | Purpose |
|------|---------|
| `--bundle <path>` | Path for the new bundle (local path or `s3://...`) |
| `-m, --message` | Commit message (auto-generated if omitted) |
| `--format json` | JSON output (default: `table`) |
| `--config <path>` | YAML/JSON config file |

### `bundlebase extend` — Mutating commands (auto-commits)

Opens an existing bundle in read-write mode. Executes the command(s) and **automatically commits** afterward. Use `-m` to provide a commit message; otherwise one is generated from the command. Multiple statements can be separated with `;` — all changes are committed together as a single commit.

```bash
bundlebase extend --bundle ./my-bundle -m "Cleaned up names" "RENAME COLUMN fname TO first_name"

# Multiple statements in one call (committed together)
bundlebase extend --bundle ./my-bundle -m "Initial cleanup" "DROP COLUMN internal_id; RENAME COLUMN fname TO first_name"

# Extend to a new directory (fork the bundle)
bundlebase extend --bundle ./source --to ./fork "FILTER WITH SELECT * FROM bundle WHERE active"
```

| Flag | Purpose |
|------|---------|
| `--bundle <path>` | Path to the existing bundle |
| `--to <path>` | Extend to a new directory instead of modifying in place |
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

Add to your MCP settings (e.g., Claude Code `mcp_servers` config). No `--bundle` needed — the agent opens bundles dynamically using tools:

```json
{
  "bundlebase": {
    "command": "bundlebase",
    "args": ["mcp"]
  }
}
```

### Available MCP Tools

Multiple bundles can be open simultaneously, each identified by a unique `bundle` name.

| Tool | Parameters | Description |
|------|------------|-------------|
| `create_bundle` | `bundle` (string), `path` (string) | Create a new bundle with the given identifier |
| `open_bundle` | `bundle` (string), `path` (string), `read_only` (bool, optional) | Open an existing bundle with the given identifier |
| `close_bundle` | `bundle` (string) | Close a bundle by its identifier |
| `list_bundles` | (none) | List all open bundles with their identifier, path, name, and description |
| `query` | `bundle` (string), `sql` (string) | Execute any SQL query or bundlebase command. Returns JSON. 1000-row limit. |
| `schema` | `bundle` (string) | Get column names, data types, and nullability |
| `count` | `bundle` (string) | Get total row count |
| `sample` | `bundle` (string), `limit` (optional, default 10) | Preview sample rows as JSON |
| `status` | `bundle` (string) | Show uncommitted changes |
| `history` | `bundle` (string) | Show commit history |

The `query` tool handles everything: SELECT queries, ATTACH, DETACH, FILTER, RENAME, COMMIT, and all other bundlebase SQL commands.

### MCP Workflow Example

```
1. Call `create_bundle` or `open_bundle` with a bundle name to load a bundle
2. Call `schema` with the bundle name to understand the data structure
3. Call `sample` with the bundle name to preview the data
4. Call `query` with the bundle name and SQL to explore and transform
5. Call `status` with the bundle name to review uncommitted changes
6. Call `query` with the bundle name and "COMMIT 'message'" to save
7. Call `close_bundle` with the bundle name when done
```

## Delegating Data Research

When using sub-agents to search for data sources, include these constraints in the delegation prompt:

> "I'm using bundlebase to build a versioned, queryable dataset. I need URLs that can be used with bundlebase's http connector: `CREATE SOURCE USING http WITH (url = '...')`. Find direct-download CSV/JSON/Parquet URLs. Do NOT test URLs with curl or wget — just find and return them. Prefer smaller, scoped datasets over huge bulk downloads. If the source supports query parameters (date range, filters), include those to keep downloads fast."

This prevents sub-agents from falling back to curl/wget and ensures the URLs work with bundlebase.

## Start Small, Then Expand

When fetching from a new data source, **start with a small scoped request** to validate the approach before downloading everything:

```bash
# BAD: Download all data at once (may be huge, may timeout)
bundlebase create --bundle ./lakes "CREATE SOURCE USING http WITH (url = 'https://api.example.com/all-data.csv')"

# GOOD: Start with a small slice to validate format and structure
bundlebase create --bundle ./lakes "CREATE SOURCE USING http WITH (url = 'https://api.example.com/data?year=2024&limit=1000')"

# Verify it worked
bundlebase query --bundle ./lakes --format json "SHOW COLUMNS"
bundlebase query --bundle ./lakes --format json "SHOW COUNT"

# Then expand to more data
bundlebase extend --bundle ./lakes "CREATE SOURCE USING http WITH (url = 'https://api.example.com/data?year=2023')"
```

If a download times out, scope it with URL query parameters (date range, geographic filter, row limit).

## Identifiers and Case Sensitivity

**Bundlebase is always case-sensitive.** Column names, join names, view names, and all other identifiers preserve their exact case. `Revenue`, `revenue`, and `REVENUE` are three different columns.

This is intentional: bundlebase works with disparate data sources (CSVs, APIs, Parquet files, databases) that each have their own case conventions. Assuming any normalization would silently break data from sources that rely on specific casing.

**Quoted identifiers:** Use double quotes for identifiers containing spaces, dots, or other special characters:
```sql
RENAME COLUMN "ResultMeasureValue" TO secchi_depth
CAST COLUMN "Measure/Unit" TO Utf8
DROP COLUMN "column with spaces"
```

Bare identifiers (no quotes) work for names containing only letters, digits, and underscores. Quotes are optional for such names — `RENAME COLUMN name TO new_name` and `RENAME COLUMN "name" TO "new_name"` are equivalent.

## Common Mistakes to Avoid

| Don't do this | Why | Do this instead |
|---------------|-----|-----------------|
| `curl`/`wget` to download data | No versioning, caching, or error handling | `CREATE SOURCE USING http WITH (url = '...')` |
| `pip install pandas` to read CSV | Extra dependency; no history tracking | `bundlebase query` for exploration |
| Materialize huge datasets in memory | Crashes on large data | Use `to_pandas()` / `to_polars()` which stream internally |
| Skip commits during exploration | Lost history, can't undo mistakes | Commit at every meaningful step |
| Create a bundle without SET NAME / SET DESCRIPTION | Bundles are hard to identify later | Always set both when creating a bundle |
| Download data then ATTACH separately | Two steps when one will do | `CREATE SOURCE USING http; FETCH bundle ADD` in one call |

## Bundle References (`bundle://`)

Use `bundle://` URLs in ATTACH and JOIN to reference another committed bundle's query output — including all its filters, column ops, and joins.

| Format | Example |
|--------|---------|
| Relative path | `bundle://./other-bundle` |
| Absolute path | `bundle:///home/user/other-bundle` |
| S3 | `bundle+s3://bucket/path/to/bundle` |
| GCS | `bundle+gcs://bucket/path/to/bundle` |

```sql
-- Join with another bundle's processed output
JOIN 'bundle://./stations' AS stations ON lake_id = stations.lake_id

-- Attach another bundle's data into yours
ATTACH 'bundle:///path/to/other/bundle'
```

The target bundle must be committed. The referenced data reflects the target's full query output at read time.

## Common Workflows

### 1. Analyze a Data File

```bash
# Create bundle and load data (always set name and description)
bundlebase create --bundle ./analysis -m "Loaded sales data" "SET NAME 'Sales Analysis'; SET DESCRIPTION 'Analysis of Q4 sales data by department'; ATTACH 'sales.csv'"

# Explore the schema
bundlebase query --bundle ./analysis --format json "SHOW COLUMNS"

# Count rows
bundlebase query --bundle ./analysis --format json "SHOW COUNT"

# Run queries
bundlebase query --bundle ./analysis --format json "SELECT department, COUNT(*) as cnt, AVG(salary) as avg_salary FROM bundle GROUP BY department ORDER BY avg_salary DESC"
```

### 2. Clean and Transform Data

```bash
# Drop unnecessary columns and rename others (committed together)
bundlebase extend --bundle ./clean -m "Removed internal columns" "DROP COLUMN internal_id; DROP COLUMN debug_notes"
bundlebase extend --bundle ./clean -m "Standardized names" "RENAME COLUMN fname TO first_name; RENAME COLUMN lname TO last_name"

# Add a computed column
bundlebase extend --bundle ./clean "ADD COLUMN full_name first_name || ' ' || last_name"

# Filter out bad data
bundlebase extend --bundle ./clean -m "Removed rows without email" "FILTER WITH SELECT * FROM bundle WHERE email IS NOT NULL"
```

### 3. Join Multiple Data Sources

```bash
# Start with a base dataset
bundlebase create --bundle ./combined "ATTACH 'customers.parquet'"

# Join with a file
bundlebase extend --bundle ./combined "JOIN 'orders.csv' AS orders ON bundle.id = orders.customer_id"

# Join with another bundle (reads the target bundle's full query output, including filters/transforms)
bundlebase extend --bundle ./combined "JOIN 'bundle://./regions' AS regions ON bundle.region_code = regions.code"

# Query across joined data
bundlebase query --bundle ./combined --format json "SELECT c.name, COUNT(orders.id) as order_count FROM bundle c JOIN orders ON c.id = orders.customer_id GROUP BY c.name ORDER BY order_count DESC LIMIT 10"

# Remove a join when no longer needed
bundlebase extend --bundle ./combined "DROP JOIN orders"
```

**`bundle://` URLs:** Use `bundle:///path` to reference another committed bundle's query output as a data source. This includes all filters, column operations, and joins applied to that bundle. For remote bundles, use `bundle+s3://bucket/path`.

### 4. Work with Multiple File Formats

```bash
# Attach multiple files in one call (committed together)
bundlebase create --bundle ./multi -m "Loaded all data files" "ATTACH 'data.csv'; ATTACH 'more_data.parquet'; ATTACH 'extra.json'"

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
bundlebase query --bundle ./reports --format json "SELECT * FROM active_users LIMIT 5"

# Drop a view
bundlebase extend --bundle ./reports "DROP VIEW high_value"
```

### 6. Full-Text Search

```bash
# Create a text index on a column
bundlebase extend --bundle ./docs "CREATE TEXT INDEX ON description"

# Search with BM25 relevance scoring
bundlebase query --bundle ./docs --format json "SELECT title, _score FROM search('description', 'machine learning') ORDER BY _score DESC LIMIT 10"

# Combine search with filters
bundlebase query --bundle ./docs --format json "SELECT * FROM search('description', 'neural networks') WHERE category = 'AI'"
```

### 7. Version Control

```bash
# View history
bundlebase query --bundle ./data --format json "SHOW HISTORY"

# View uncommitted changes
bundlebase query --bundle ./data --format json "SHOW STATUS"

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
bundlebase extend --bundle ./pipeline "CREATE SOURCE USING my_connector WITH (url = 's3://bucket/data/')"

# Preview what fetch would do (dry run)
bundlebase query --bundle ./pipeline --format json "FETCH bundle ADD DRY RUN"

# Actually fetch new files
bundlebase extend --bundle ./pipeline "FETCH bundle ADD"

# Fetch all sources
bundlebase extend --bundle ./pipeline "FETCH ALL SYNC"
```

### 10. Bundle Metadata

```bash
# Set bundle name and description (committed together)
bundlebase extend --bundle ./data "SET NAME 'Q4 Sales Report'; SET DESCRIPTION 'Quarterly sales data with regional breakdowns'"

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
bundlebase query --bundle s3://mybucket/my-bundle --format json "SELECT COUNT(*) FROM bundle"

# Read-only schema check
bundlebase query --bundle s3://mybucket/my-bundle --format json "SHOW COLUMNS"
```

## Fetching External Data with Connectors

Bundlebase has built-in connectors for common data sources. The pattern is: CREATE SOURCE → FETCH → query/transform.

**Do NOT use curl, wget, or requests to download data files.** Use bundlebase connectors instead — they handle downloading, format detection, versioning, and caching automatically.

**Choosing a connector:**

| Data source | Connector | Example |
|------------|-----------|---------|
| Any URL (CSV, JSON, Parquet) | `http` | `CREATE SOURCE USING http WITH (url = 'https://...')` |
| Kaggle dataset | `kaggle` | `CREATE SOURCE USING kaggle WITH (dataset = 'owner/name')` |
| S3/GCS/Azure/local directory | `remote_dir` | `CREATE SOURCE USING remote_dir WITH (url = 's3://...')` |
| FTP server | `ftp_directory` | `CREATE SOURCE USING ftp_directory WITH (url = 'ftp://...')` |
| SFTP server | `sftp_directory` | `CREATE SOURCE USING sftp_directory WITH (url = 'sftp://...')` |
| Webpage with file links | `web_scrape` | `CREATE SOURCE USING web_scrape WITH (url = 'https://...')` |
| PostgreSQL database | `postgres` | `CREATE SOURCE USING postgres WITH (url = 'postgres://...')` |
| Custom API with pagination/auth | Python connector | See "Building a Custom Connector" below |

```bash
# URL: download CSV/JSON/Parquet from any HTTP(S) URL
bundlebase create --bundle ./lake-data "CREATE SOURCE USING http WITH (url = 'https://data.mn.gov/api/lake_quality.csv')"

# Kaggle: fetch a public dataset (requires ~/.kaggle/kaggle.json)
bundlebase create --bundle ./housing "CREATE SOURCE USING kaggle WITH (dataset = 'zillow/zecon', patterns = '*.csv')"

# S3: attach all parquet files from a bucket
bundlebase create --bundle ./logs "CREATE SOURCE USING remote_dir WITH (url = 's3://my-bucket/data/', patterns = '**/*.parquet')"

# Preview what would be fetched without actually fetching
bundlebase query --bundle ./logs --format json "FETCH bundle ADD DRY RUN"
```

**Kaggle note:** The `dataset` parameter uses the `owner/dataset-name` format from the Kaggle URL. For example, `kaggle.com/datasets/tunguz/200000-jeopardy-questions` → `dataset = 'tunguz/200000-jeopardy-questions'`. The kaggle connector calls the Kaggle REST API directly — no need to install the `kaggle` pip package. It only requires `~/.kaggle/kaggle.json` (create at kaggle.com → Settings → API → Create New Token).

Use `SYNTAX CREATE SOURCE` and `SYNTAX FETCH` for detailed syntax.

### Iterative multi-source dataset building

When working with multiple data sources, build each one as a separate bundle first — explore, clean, and reshape independently. Then combine them.

**Step 1: Build separate bundles for each data source**

```bash
# Bundle 1: Lake quality measurements
bundlebase create --bundle ./lakes "CREATE SOURCE USING http WITH (url = 'https://data.mn.gov/lakes.csv')"
bundlebase extend --bundle ./lakes "RENAME COLUMN lake_identifier TO lake_id"
bundlebase extend --bundle ./lakes -m "Cleaned lake data" "FILTER WITH SELECT * FROM bundle WHERE measurement_date > '2020-01-01'"

# Bundle 2: Monitoring stations
bundlebase create --bundle ./stations "CREATE SOURCE USING http WITH (url = 'https://data.mn.gov/stations.csv')"
bundlebase extend --bundle ./stations -m "Kept active stations" "FILTER WITH SELECT * FROM bundle WHERE status = 'active'"
```

**Step 2: Explore each bundle, verify data quality**

```bash
bundlebase query --bundle ./lakes --format json "SHOW COLUMNS"
bundlebase query --bundle ./stations --format json "SELECT * FROM bundle LIMIT 5"
```

**Step 3: Combine using `bundle://` joins or `IMPORT JOIN`**

*Option A: Live join via `bundle://`* — the join references the other bundle at query time. No data copying. Good for exploration.

```bash
bundlebase extend --bundle ./lakes "JOIN 'bundle://./stations' AS stations ON lake_id = stations.lake_id"
bundlebase query --bundle ./lakes --format json "SELECT * FROM bundle JOIN stations ON lake_id = stations.lake_id LIMIT 10"
```

*Option B: Solidify the join* — copies all data, commits, and indexes into one self-contained bundle. Good for the final dataset. Requires the `bundle://` join from Option A first.

```bash
bundlebase extend --bundle ./lakes "IMPORT JOIN stations"
```

**When to use which:**

| Approach | Use when |
|----------|----------|
| `bundle://` JOIN | Exploring, prototyping, data may still change |
| `IMPORT JOIN` | Finalizing — want self-contained bundle with full commit history from the source |
| `IMPORT JOIN ... FLATTEN HISTORY` | Same, but collapse all imported commits into a single commit for a cleaner history |

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
bundlebase extend --bundle ./data "CREATE SOURCE USING my.api"
bundlebase extend --bundle ./data "FETCH bundle ADD"
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
bundlebase extend --bundle ./data -m "Cleaned and enriched data" "ADD COLUMN clean_phone AS util.normalize_phone(phone)"
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
bundlebase query --bundle ./analysis "EXPORT TO 'output.csv' SELECT * FROM bundle"

# Export filtered results to JSON Lines
bundlebase query --bundle ./analysis "EXPORT TO 'active_users.jsonl' SELECT * FROM bundle WHERE active = true"

# Export aggregated results
bundlebase query --bundle ./analysis "EXPORT TO 'summary.csv' SELECT department, COUNT(*) as cnt, AVG(salary) as avg_sal FROM bundle GROUP BY department"
```

**Supported formats:** `.csv`, `.jsonl` (JSON Lines — one JSON object per line)

**Tip:** For data exploration where you need to see results, use `SELECT` with `--format json`. For saving data to a file for further processing, prefer `EXPORT TO` — it streams directly to the file without row limits.

### Sharing Bundles

Bundles are portable — share them with teammates:

```bash
# Push a bundle to S3 so others can access it
bundlebase create --bundle s3://team-bucket/shared-analysis -m "Shared cleaned dataset" "ATTACH 'cleaned.parquet'"

# Others can then query it
bundlebase query --bundle s3://team-bucket/shared-analysis --format json "SHOW COLUMNS"
```

## SQL Reference Summary

The table name for bundle data is always `bundle`. Standard SQL (Apache DataFusion syntax) is supported for SELECT queries.

**String quoting in SQL:** Use single quotes for string values. To include a literal single quote, double it: `'O''Brien'`. Example: `COMMIT 'Added O''Brien data'`.

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
| `DELETE FROM bundle WHERE ...` | Delete rows matching a condition |
| `ALWAYS DELETE FROM bundle WHERE ...` | Persistent rule: auto-delete matching rows on every future ATTACH |
| `DROP ALWAYS DELETE [WHERE ...]` | Remove one or all always-delete rules |
| `SHOW ALWAYS DELETES` | List active always-delete rules |
| `DESCRIBE DATA IN col1, col2` | Profile columns (min/max/avg/nulls/top values) |
| `DESCRIBE DATA IN col AS TYPE` | Detect sentinel values that fail to cast |

These SQL commands also work with `bundlebase query`, e.g. `bundlebase query --bundle ./data --format json "SHOW COLUMNS"`.
