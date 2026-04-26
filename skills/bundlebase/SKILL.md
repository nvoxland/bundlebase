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

**Check if bundlebase MCP tools are available before deciding which mode to use.**

Bundlebase has two modes: CLI commands (run via shell) and MCP tools (direct function calls like `mcp__bundlebase__query`). Both work, but they have different strengths.

**Decision rule:**
1. **Single operation** (one query, one rename, one attach) → **CLI is fine.** It's simple and self-contained.
2. **Multi-step work** (exploration, analysis, building up a bundle) → **Use MCP if available.** MCP keeps bundles open with cached state across calls — no re-opening, no shell overhead, and you can hold multiple bundles open simultaneously.
3. **MCP tools not available** → use CLI for everything.

**Use CLI for:**
- `bundlebase list-bundles` to discover bundles in a directory
- A single query: `bundlebase query --bundle ./data "SELECT COUNT(*) FROM bundle"`
- A single mutation: `bundlebase extend --bundle ./data "RENAME COLUMN x TO y"`

**Use MCP for:**
- Any workflow involving more than one operation on the same bundle
- Exploration (schema → sample → query → query → ...)
- Data transformation (attach → rename → filter → commit)
- Cross-bundle analysis with multiple sources open at once
- Any iterative or analytical work

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

**Dollar-quoting (`$$...$$`):** For strings that contain single quotes, newlines, or JSON, use PostgreSQL-style dollar-quoting — no escaping needed. The content between `$$` delimiters is taken exactly as written:

```bash
# Single quote in a commit message — no escaping needed
bundlebase extend --bundle ./data "COMMIT \$\$fixed the connector's bug\$\$"
```

In MCP SQL (no shell escaping needed):
```sql
-- Commit message with a single quote
COMMIT $$fixed the connector's bug$$

-- Multi-line JSON body for a POST request
CREATE SOURCE USING http WITH (
    url = 'https://api.example.com/query',
    method = 'POST',
    body = $${
        "filters": {"state": "MN", "type": "Lake"},
        "format": "csv"
    }$$,
    headers = 'Content-Type: application/json
Accept: text/csv'
)
```

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

**This is the preferred way to use bundlebase.** MCP tools are available as `mcp__bundlebase__*` function calls when the MCP server is configured. They keep bundles open across calls, preserving cache and state.

### MCP Server Configuration

Add to your MCP settings (e.g., Claude Code `mcp_servers` config):

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

| MCP Tool Name | Parameters | Description |
|------|------------|-------------|
| `mcp__bundlebase__create_bundle` | `bundle` (string), `path` (string) | Create a new bundle with the given identifier |
| `mcp__bundlebase__open_bundle` | `bundle` (string), `path` (string), `read_only` (bool, optional) | Open an existing bundle with the given identifier |
| `mcp__bundlebase__close_bundle` | `bundle` (string) | Close a bundle by its identifier |
| `mcp__bundlebase__list_bundles` | (none) | List all open bundles with their identifier, path, name, and description |
| `mcp__bundlebase__query` | `bundle` (string), `sql` (string) | Execute any SQL query or bundlebase command. Returns JSON. 1000-row limit. |
| `mcp__bundlebase__schema` | `bundle` (string) | Get column names, data types, and nullability |
| `mcp__bundlebase__count` | `bundle` (string) | Get total row count |
| `mcp__bundlebase__sample` | `bundle` (string), `limit` (optional, default 10) | Preview sample rows as JSON |
| `mcp__bundlebase__status` | `bundle` (string) | Show uncommitted changes |
| `mcp__bundlebase__history` | `bundle` (string) | Show commit history |
| `mcp__bundlebase__generate_report` | `input` (string), `output` (string), `branding` (bool, optional) | Generate a PDF report from markdown with embedded data queries and charts |

The `query` tool handles everything: SELECT queries, ATTACH, DETACH, FILTER, RENAME, COMMIT, and all other bundlebase SQL commands.

### Progress Tracking

The `mcp__bundlebase__query` tool supports the MCP `progressToken` protocol — when you pass a `_meta.progressToken`, the server emits `notifications/progress` updates during execution so the user can see what's happening in real time.

**Always pass a progress token for every query.** Any operation could be slow depending on data size — `FETCH`, `ATTACH`, `CREATE INDEX`, `CAST COLUMN`, and others can all take seconds to minutes. Passing a token that goes unused has no cost; missing one on a slow operation leaves the user with no feedback.

```
# Claude Code / MCP framework handles the token transparently when you call:
mcp__bundlebase__query(bundle="data", sql="FETCH base SYNC")
```

The progress notifications include:
- Operation name (e.g., "Processing 42 discovered files")
- Current count and total (e.g., `progress: 12, total: 42`)
- File or item currently being processed

**If no progress notifications arrive within ~10 seconds of starting a slow operation:**
1. The operation may still be running (some sources are slow to start)
2. The client may not support `progressToken` — the operation still completes, you just won't see live updates
3. If the tool call itself hangs (no response after several minutes), see the recovery section below

### Recovering from Stalled Operations

If a `mcp__bundlebase__query` call appears stuck (tool call never returns, or returns after an unreasonably long time with no result):

**Step 1 — Check if the operation actually completed:**
```
mcp__bundlebase__status(bundle="data")
```
If status shows the expected changes (e.g., an ATTACH or FETCH), the operation completed — the result just wasn't returned. Commit normally.

**Step 2 — If status shows nothing pending, check history:**
```
mcp__bundlebase__query(bundle="data", sql="SHOW HISTORY")
```
If the operation was committed before the hang (e.g., an auto-commit source creation), it's done.

**Step 3 — If the bundle is in an unknown state, use a dry run to assess:**
```
mcp__bundlebase__query(bundle="data", sql="FETCH base ADD DRY RUN")
```
Dry run shows what *would* be fetched without making changes. If it shows 0 new files, the data was already fetched.

**Step 4 — If the MCP server itself is unresponsive**, close and reopen the bundle:
```
mcp__bundlebase__close_bundle(bundle="data")
mcp__bundlebase__open_bundle(bundle="data", path="./data")
mcp__bundlebase__status(bundle="data")
```
Opening an existing bundle restores committed state. Any uncommitted changes from before the hang are gone — use `SHOW HISTORY` to see the last committed version.


### MCP Workflow Example

```
1. mcp__bundlebase__create_bundle(bundle="sales", path="./sales-data")
   — or mcp__bundlebase__open_bundle(bundle="sales", path="./sales-data")
2. mcp__bundlebase__query(bundle="sales", sql="SET NAME 'Sales Data'; SET DESCRIPTION 'Q4 sales records'")
3. mcp__bundlebase__query(bundle="sales", sql="ATTACH 'sales.csv'")
4. mcp__bundlebase__schema(bundle="sales")
5. mcp__bundlebase__sample(bundle="sales")
6. mcp__bundlebase__query(bundle="sales", sql="SELECT department, COUNT(*) as cnt FROM bundle GROUP BY department")
7. mcp__bundlebase__query(bundle="sales", sql="RENAME COLUMN fname TO first_name")
8. mcp__bundlebase__query(bundle="sales", sql="COMMIT 'Loaded and cleaned sales data'")
9. mcp__bundlebase__close_bundle(bundle="sales")
```

Notice how steps 3-7 all reuse the same open bundle — no re-opening, no `--bundle` flag, no shell overhead. This is why MCP is preferred for any multi-step workflow.

### Verify Before Committing

In MCP mode, operations are **uncommitted** until you explicitly run `COMMIT`. This means you can test your changes before making them permanent.

**`SHOW STATUS`** is your key tool for understanding uncommitted state. It lists every pending change with an id, description, and operation count. Use it to:
- See what's pending before committing
- Verify an operation was recorded after running it
- Confirm an UNDO removed what you expected

**Workflow for checking and rolling back:**

```
# Check current uncommitted state
mcp__bundlebase__query(bundle="data", sql="SHOW STATUS")
# Returns a table of pending changes, e.g.:
#   id | description              | operation_count
#   0  | ATTACH 'sales.csv'       | 2
#   1  | RENAME COLUMN x TO y     | 1
#   2  | FETCH base ADD          | 3

# Something went wrong with the fetch — undo just that last change
mcp__bundlebase__query(bundle="data", sql="UNDO")

# Verify it was removed
mcp__bundlebase__query(bundle="data", sql="SHOW STATUS")
# Now shows only changes 0 and 1

# The earlier changes are still intact — query the data to verify
mcp__bundlebase__sample(bundle="data")

# If everything looks good, commit
mcp__bundlebase__query(bundle="data", sql="COMMIT 'Loaded and renamed columns'")
```

**UNDO vs RESET:**
- `UNDO` — removes the last uncommitted change only. Returns the command that was undone (e.g., `"UNDONE: RENAME COLUMN x TO y"`).
- `UNDO LAST N` — removes the last N uncommitted changes at once. Errors if N exceeds the number of available changes.
- `RESET` — discards ALL uncommitted changes at once, reverting to the last committed state. Use when you want to start over completely.

**Only commit once you've verified the results.** This check-then-commit cycle is the key advantage of MCP over CLI — use it.

**WARNING about CLI `extend`:** The `bundlebase extend` command **auto-commits after every call**. There is no way to undo a committed change (short of recreating the bundle). This is why CLI is only appropriate for single, well-understood operations. For any workflow where you might need to check results or iterate, use MCP.

### Working with Multiple Bundles and Cross-Bundle Analysis

MCP can hold multiple bundles open simultaneously. Use this to keep data sources separate during exploration, then combine them on demand.

**Best practice: one bundle per data source during exploration.** Create a separate bundle for each data source you're pulling in. This lets you explore, clean, and understand each source independently before combining them.

When you need to query across sources, create a temporary `memory://` bundle that joins the source bundles together. The `memory://` bundle exists only in memory — no files on disk — so it's perfect for ad-hoc cross-source analysis. Close it when you're done.

**Multi-bundle exploration workflow:**

```
# Step 1: Create separate bundles for each data source
mcp__bundlebase__create_bundle(bundle="customers", path="./customers")
mcp__bundlebase__query(bundle="customers", sql="SET NAME 'Customers'; SET DESCRIPTION 'Customer records from CRM export'")
mcp__bundlebase__query(bundle="customers", sql="ATTACH 'customers.csv'")
mcp__bundlebase__query(bundle="customers", sql="COMMIT 'Loaded customer data'")

mcp__bundlebase__create_bundle(bundle="orders", path="./orders")
mcp__bundlebase__query(bundle="orders", sql="SET NAME 'Orders'; SET DESCRIPTION 'Order history from fulfillment system'")
mcp__bundlebase__query(bundle="orders", sql="ATTACH 'orders.csv'")
mcp__bundlebase__query(bundle="orders", sql="COMMIT 'Loaded order data'")

# Step 2: Explore each source independently
mcp__bundlebase__schema(bundle="customers")
mcp__bundlebase__sample(bundle="customers")
mcp__bundlebase__schema(bundle="orders")
mcp__bundlebase__sample(bundle="orders")

# Step 3: Create a temporary memory bundle to query across sources
mcp__bundlebase__create_bundle(bundle="analysis", path="memory://")
mcp__bundlebase__query(bundle="analysis", sql="ATTACH 'bundle://./customers'")
mcp__bundlebase__query(bundle="analysis", sql="JOIN 'bundle://./orders' AS orders ON bundle.id = orders.customer_id")

# Step 4: Run cross-source queries against the temporary bundle
mcp__bundlebase__query(bundle="analysis", sql="SELECT name, COUNT(orders.order_id) as order_count, SUM(orders.total) as lifetime_value FROM bundle JOIN orders ON bundle.id = orders.customer_id GROUP BY name ORDER BY lifetime_value DESC LIMIT 20")

# Step 5: Close the temporary bundle when done (nothing to commit — it's in-memory)
mcp__bundlebase__close_bundle(bundle="analysis")

# The source bundles remain open for further work
mcp__bundlebase__query(bundle="customers", sql="RENAME COLUMN fname TO first_name")
```

**When to use this pattern:**
- Pulling data from multiple APIs, files, or Kaggle datasets
- Exploring how different data sources relate before deciding on a final schema
- Running ad-hoc analytical queries that span multiple sources
- Comparing or validating data across sources

**Key points:**
- `memory://` bundles have no disk footprint — create and discard them freely
- `bundle://./path` references read committed data from another bundle at query time
- Source bundles stay open and independent — changes to one don't affect others
- If the analysis proves useful, you can create a permanent bundle with the same joins instead of using `memory://`

## Delegating Data Research

When using sub-agents to search for data sources, include these constraints in the delegation prompt:

> "I'm using bundlebase to build a versioned, queryable dataset. I need URLs that can be used with bundlebase's http connector: `CREATE SOURCE USING http WITH (url = '...')`. Find direct-download CSV/TSV/JSON/Parquet URLs. Do NOT test URLs with curl or wget — just find and return them. Prefer smaller, scoped datasets over huge bulk downloads. If the source supports query parameters (date range, filters), include those to keep downloads fast."

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

## Important: Don't Drop Columns Preemptively

Do NOT drop columns just because they aren't used in the current analysis. Bundles are designed for reuse — columns that seem irrelevant now may be valuable for future analysis, joins, or other users. Only drop columns when the user explicitly asks to remove them.

## Operation Ordering

When cleaning data, always fix bad values or filter out bad rows BEFORE adding computed columns or casting. Otherwise CAST or arithmetic may fail on dirty data. Use `TRY_CAST` instead of `CAST` for defensive type conversion in ADD COLUMN expressions.

```
# BAD: computed column fails because some rows have non-numeric values
mcp__bundlebase__query(bundle="data", sql="ADD COLUMN price_cents AS CAST(price AS DOUBLE) * 100")

# GOOD: profile the data first, then fix bad values, then compute
mcp__bundlebase__query(bundle="data", sql="PROFILE COLUMN price FOR CAST TO Float64")
mcp__bundlebase__query(bundle="data", sql="UPDATE bundle SET price = NULL WHERE TRY_CAST(price AS Float64) IS NULL")
mcp__bundlebase__query(bundle="data", sql="ADD COLUMN price_cents AS CAST(price AS Float64) * 100")
```

## Data Quality Checks

**Always profile columns before casting them.** This reveals dirty data, sentinel values, NULLs, and type mismatches that would cause CAST failures.

```
# Profile columns to see min/max/avg/nulls/top values — do this FIRST
mcp__bundlebase__query(bundle="data", sql="DESCRIBE DATA IN price, quantity")

# Before casting a text column to a number, use DESCRIBE DATA ... AS TYPE
# to find values that won't convert (e.g., "N/A", ">2", "10,5")
mcp__bundlebase__query(bundle="data", sql="DESCRIBE DATA IN price AS DOUBLE")

# Use PROFILE COLUMN for a focused view of a single column's values and frequencies
mcp__bundlebase__query(bundle="data", sql="PROFILE COLUMN price")

# Use PROFILE COLUMN ... FOR CAST TO to see exactly which values can't be cast
mcp__bundlebase__query(bundle="data", sql="PROFILE COLUMN price FOR CAST TO Float64")
# Returns a table of (value, count) for all non-castable values

# Then fix or remove the bad values before casting (see "Cleaning Data" below)
mcp__bundlebase__query(bundle="data", sql="UPDATE bundle SET price = NULL WHERE price IN ('N/A', '-', '')")
mcp__bundlebase__query(bundle="data", sql="CAST COLUMN price TO Float64")
```

**The pattern for type conversion:**
1. `PROFILE COLUMN col FOR CAST TO TARGET_TYPE` — find values that won't cast (with counts)
2. `UPDATE` to fix bad values, or `DELETE` to remove bad rows
3. `CAST COLUMN col TO TARGET_TYPE` — now safe to cast (pre-flight check runs automatically)

## Understanding CAST COLUMN — Runtime Behavior

**Critical:** `CAST COLUMN` is a lazy operation. It does not transform data in-place — it adds a cast expression applied at query time. This means:

- **Verification happens at cast time:** By default, `CAST COLUMN` runs a pre-flight scan of existing data to detect non-castable values before recording the operation. If any are found, it aborts with an error listing examples and a `PROFILE COLUMN` suggestion.
- **Future blocks are not pre-verified:** When you later attach new blocks of data, those new rows are NOT pre-checked. If new data contains non-castable values, any SELECT query on that column will fail at runtime.
- **Error recovery:** If a cast fails at runtime (e.g., on newly attached data), the error message will suggest running `PROFILE COLUMN` to find the bad values. Use `UPDATE`/`DELETE` to fix them, then retry.

```
# Default: verifies existing data before recording the cast
mcp__bundlebase__query(bundle="data", sql="CAST COLUMN value TO Float64")
# → Aborts if any existing values can't be cast, with a PROFILE COLUMN suggestion

# Skip pre-flight check (faster, but you accept the risk of runtime cast errors):
mcp__bundlebase__query(bundle="data", sql="CAST COLUMN value TO Float64 NO VERIFY EXISTING")

# Revert a cast if you made a mistake:
mcp__bundlebase__query(bundle="data", sql="DROP CAST COLUMN value")
```

**Handling future data that may not always be castable:**

If new blocks of data will be attached in the future and you can't guarantee they'll be castable, use one of these approaches:

1. **ALWAYS UPDATE / ALWAYS DELETE** — persistent rules applied to every new block:
   ```
   # Null out non-castable values in every future block
   mcp__bundlebase__query(bundle="data", sql="ALWAYS UPDATE bundle SET value = NULL WHERE TRY_CAST(value AS Float64) IS NULL AND value IS NOT NULL")
   
   # Or delete rows with bad values from every future block
   mcp__bundlebase__query(bundle="data", sql="ALWAYS DELETE FROM bundle WHERE TRY_CAST(value AS Float64) IS NULL AND value IS NOT NULL")
   ```

2. **ADD COLUMN with TRY_CAST instead of CAST COLUMN** — keeps the original and adds a derived column. NULL values show you which originals failed to cast:
   ```
   # This is the safest approach when data quality is uncertain
   mcp__bundlebase__query(bundle="data", sql="ADD COLUMN value_float AS TRY_CAST(value AS Float64)")
   # Now you can see value_float (NULL where cast failed) alongside the original value
   # Find which values are failing: SELECT value, COUNT(*) FROM bundle WHERE value_float IS NULL GROUP BY value
   ```

## Cleaning Data with UPDATE and DELETE

Use `UPDATE` and `DELETE` for targeted data cleaning instead of rebuilding the entire bundle. These are more precise than FILTER for fixing specific issues.

```
# Fix known sentinel values
mcp__bundlebase__query(bundle="data", sql="UPDATE bundle SET price = NULL WHERE price = 'N/A'")
mcp__bundlebase__query(bundle="data", sql="UPDATE bundle SET price = NULL WHERE price = '-'")

# Standardize formats (e.g., European decimals)
mcp__bundlebase__query(bundle="data", sql="UPDATE bundle SET price = REPLACE(price, ',', '.') WHERE price LIKE '%,%'")

# Trim whitespace
mcp__bundlebase__query(bundle="data", sql="UPDATE bundle SET name = TRIM(name)")

# Standardize categorical values
mcp__bundlebase__query(bundle="data", sql="UPDATE bundle SET status = 'active' WHERE LOWER(status) IN ('active', 'act', 'a', 'yes')")

# Delete rows that are clearly invalid
mcp__bundlebase__query(bundle="data", sql="DELETE FROM bundle WHERE created_date IS NULL AND name IS NULL")

# Delete duplicates (keep first occurrence)
mcp__bundlebase__query(bundle="data", sql="DELETE FROM bundle WHERE rowid NOT IN (SELECT MIN(rowid) FROM bundle GROUP BY email)")

# Use ALWAYS DELETE for rules that should apply to future data too
mcp__bundlebase__query(bundle="data", sql="ALWAYS DELETE FROM bundle WHERE test_record = true")
```

**When to use UPDATE vs DELETE vs FILTER:**

| Operation | Use when |
|-----------|----------|
| `UPDATE` | Fixing values in-place — standardizing formats, replacing sentinels, trimming whitespace |
| `DELETE` | Removing specific bad rows while keeping the rest |
| `FILTER` | Keeping only rows that match a condition (inverse of DELETE, but creates a new filtered view of all data) |
| `ALWAYS DELETE` | A persistent rule that should also apply to data attached in the future |

## Shell Escaping

When using CLI mode in zsh, `!` in double-quoted strings is expanded by the shell. Use `<>` instead of `!=` in SQL, or wrap the SQL in single quotes:

```bash
# BAD: zsh expands ! in double quotes
bundlebase query --bundle ./data "SELECT * FROM bundle WHERE status != 'active'"

# GOOD: use <> operator instead
bundlebase query --bundle ./data "SELECT * FROM bundle WHERE status <> 'active'"

# GOOD: use single quotes (but then escape inner single quotes)
bundlebase query --bundle ./data 'SELECT * FROM bundle WHERE status != '"'"'active'"'"''
```

This is not an issue in MCP mode — another reason to prefer MCP for complex queries.

## Common Mistakes to Avoid

| Don't do this                                      | Why                                                                                                                             | Do this instead                                           |
|----------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------|
| `curl`/`wget` to download data                     | No versioning, caching, or error handling                                                                                       | `CREATE SOURCE USING http WITH (url = '...')`             |
| `pip install pandas` to read CSV                   | Extra dependency; no history tracking                                                                                           | `bundlebase query` for exploration                        |
| Materialize huge datasets in memory                | Crashes on large data                                                                                                           | Use `to_pandas()` / `to_polars()` which stream internally |
| Skip commits during exploration                    | Lost history, can't undo mistakes                                                                                               | Commit at every meaningful step                           |
| Create a bundle without SET NAME / SET DESCRIPTION | Bundles are hard to identify later                                                                                              | Always set both when creating a bundle                    |
| Run FETCH after CREATE SOURCE for initial load     | CREATE SOURCE auto-fetches on creation — no separate FETCH needed for the first load                                            | Use FETCH later only to pick up new/changed data          |
| Download data then ATTACH separately               | Directly downloading loses the history of where the data came from. Only use ATTACH for data that already existed on the system | `CREATE SOURCE USING http WITH (url = ...)                |

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

**MCP workflow (preferred for multi-step cleaning):**

```
# Step 0: Normalize column names — do this immediately after loading external data
# Converts verbose names like "MonitoringLocationIdentifier" to "monitoring_location_identifier"
# This eliminates the need for double-quoting in all subsequent queries
mcp__bundlebase__query(bundle="data", sql="NORMALIZE COLUMN NAMES")
mcp__bundlebase__query(bundle="data", sql="COMMIT 'Normalized column names'")

# Step 1: Profile the data to understand what needs cleaning
mcp__bundlebase__query(bundle="data", sql="DESCRIBE DATA IN price, status, email")

# Step 2: Check columns before casting — find values that won't convert
mcp__bundlebase__query(bundle="data", sql="PROFILE COLUMN price FOR CAST TO Float64")
# Returns table of (value, count) for all non-castable values

# Step 3: Fix bad values with UPDATE
mcp__bundlebase__query(bundle="data", sql="UPDATE bundle SET price = NULL WHERE price IN ('N/A', '-', '')")
mcp__bundlebase__query(bundle="data", sql="UPDATE bundle SET price = REPLACE(price, ',', '.') WHERE price LIKE '%,%'")
mcp__bundlebase__query(bundle="data", sql="COMMIT 'Fixed price column values'")

# Step 4: Cast (pre-flight check verifies existing data automatically)
mcp__bundlebase__query(bundle="data", sql="CAST COLUMN price TO Float64")
# If you made a mistake: DROP CAST COLUMN price

# Step 5: Delete bad rows
mcp__bundlebase__query(bundle="data", sql="DELETE FROM bundle WHERE email IS NULL AND name IS NULL")

# Step 6: Rename and add computed columns
mcp__bundlebase__query(bundle="data", sql="RENAME COLUMN fname TO first_name")
mcp__bundlebase__query(bundle="data", sql="RENAME COLUMN lname TO last_name")
mcp__bundlebase__query(bundle="data", sql="ADD COLUMN full_name AS first_name || ' ' || last_name")

# Step 7: VERIFY before committing — query the uncommitted state to check your work
mcp__bundlebase__sample(bundle="data")
mcp__bundlebase__query(bundle="data", sql="SELECT COUNT(*) as rows FROM bundle")
# If the last operation looks wrong, UNDO reverts just that one change:
# mcp__bundlebase__query(bundle="data", sql="UNDO")
# Or undo the last N changes at once:
# mcp__bundlebase__query(bundle="data", sql="UNDO LAST 3")
# If everything is wrong, RESET discards ALL uncommitted changes:
# mcp__bundlebase__query(bundle="data", sql="RESET")

# Step 8: Commit only after verifying
mcp__bundlebase__query(bundle="data", sql="COMMIT 'Cleaned and standardized columns'")
```

**CLI equivalent (for simple one-off changes):**

```bash
bundlebase extend --bundle ./clean -m "Standardized names" "RENAME COLUMN fname TO first_name; RENAME COLUMN lname TO last_name"
bundlebase extend --bundle ./clean "ADD COLUMN full_name AS first_name || ' ' || last_name"
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

### 5. Save Useful Queries as Views

When you discover an interesting or potentially reusable transformation during exploration, save it as a view. Views are named queries stored in the bundle — they don't duplicate data, but let you (or future users/agents) reuse the transformation without remembering the SQL.

**When to create a view:**
- You've written a query that filters, aggregates, or reshapes data in a useful way
- You find yourself running the same query pattern repeatedly
- You want to capture a meaningful subset of the data for others to use
- You've built a complex join or transformation that would be hard to reconstruct

**MCP example — exploratory workflow that leads to views:**

```
# Exploring sales data...
mcp__bundlebase__query(bundle="sales", sql="SELECT region, COUNT(*) as orders, SUM(total) as revenue FROM bundle GROUP BY region ORDER BY revenue DESC")
# ^ This is useful — save it as a view

mcp__bundlebase__query(bundle="sales", sql="CREATE VIEW revenue_by_region AS SELECT region, COUNT(*) as orders, SUM(total) as revenue FROM bundle GROUP BY region")

# Now you or any future agent can just query the view
mcp__bundlebase__query(bundle="sales", sql="SELECT * FROM revenue_by_region WHERE revenue > 100000")

# Create more views as you find useful slices
mcp__bundlebase__query(bundle="sales", sql="CREATE VIEW active_customers AS SELECT * FROM bundle WHERE last_order_date > '2025-01-01'")
mcp__bundlebase__query(bundle="sales", sql="CREATE VIEW high_value_orders AS SELECT * FROM bundle WHERE total > 500")

# Views compose — query one view from another
mcp__bundlebase__query(bundle="sales", sql="SELECT region, AVG(total) FROM high_value_orders GROUP BY region")

# List existing views
mcp__bundlebase__query(bundle="sales", sql="SHOW VIEWS")

# Commit to persist the views
mcp__bundlebase__query(bundle="sales", sql="COMMIT 'Added analytical views'")

# Drop a view if no longer needed
mcp__bundlebase__query(bundle="sales", sql="DROP VIEW high_value_orders")
```

**CLI equivalent:**

```bash
bundlebase extend --bundle ./reports "CREATE VIEW active_users AS SELECT * FROM bundle WHERE status = 'active'"
bundlebase query --bundle ./reports --format json "SELECT * FROM active_users LIMIT 5"
bundlebase extend --bundle ./reports "DROP VIEW active_users"
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

**MCP (preferred — UNDO and RESET only work on uncommitted changes):**

```
# View history
mcp__bundlebase__query(bundle="data", sql="SHOW HISTORY")

# View uncommitted changes
mcp__bundlebase__query(bundle="data", sql="SHOW STATUS")

# Undo the last uncommitted change (keeps earlier uncommitted changes)
mcp__bundlebase__query(bundle="data", sql="UNDO")

# Or undo the last N changes at once
mcp__bundlebase__query(bundle="data", sql="UNDO LAST 3")

# Discard ALL uncommitted changes (back to last committed state)
mcp__bundlebase__query(bundle="data", sql="RESET")

# Verify data integrity
mcp__bundlebase__query(bundle="data", sql="VERIFY DATA")
```

**CLI:**

```bash
# View history and status (read-only — works with query)
bundlebase query --bundle ./data --format json "SHOW HISTORY"
bundlebase query --bundle ./data --format json "SHOW STATUS"

# Verify data integrity
bundlebase query --bundle ./data "VERIFY DATA"
```

**Note:** `UNDO` and `RESET` are not useful with CLI `extend` because extend auto-commits — there are no uncommitted changes to undo. Use MCP for workflows where you need to test and undo.

### 8. Indexes for Performance

```bash
# Create a btree index for faster filtering
bundlebase extend --bundle ./data "CREATE BTREE INDEX ON customer_id"

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
bundlebase query --bundle ./pipeline --format json "FETCH base ADD DRY RUN"

# Actually fetch new files
bundlebase extend --bundle ./pipeline "FETCH base ADD"

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

### 13. Generate PDF Reports

When the user asks for a report, use `generate_report` to create a PDF. **Do NOT write static markdown tables or embed query results as text.** Instead, use ` ```bundlebase ` fenced YAML code blocks to define live queries — the report engine executes them and renders proper styled tables and charts automatically.

**IMPORTANT:** Every data table and chart in the report MUST be a ` ```bundlebase ` code block with a `bundle`, `query`, and `type` field. Do not copy-paste query results into the markdown. The report engine handles:
- Running the SQL queries against open bundles
- Rendering tables with headers, zebra striping, and borders
- Rendering charts (pie, bar, line, etc.) with proper styling
- Formatting numbers and handling nulls

Write the narrative text as regular markdown (headings, paragraphs, bullet points). Use ` ```bundlebase ` blocks for all data.

**Available chart types:** `table`, `pie`, `bar`, `line`, `horizontal_bar`, `box_whisker`, `pyramid`, `error_bar`, `violin`

**MCP example — generate a sales report:**

```
# First, make sure the bundle is open
mcp__bundlebase__open_bundle(bundle="sales", path="./sales-data")

# Generate a PDF report with tables and charts
mcp__bundlebase__generate_report(
  output="sales-report.pdf",
  input="""
# Q4 Sales Report

## Revenue by Region

```bundlebase
bundle: sales
query: "SELECT region, SUM(revenue) as total_revenue FROM bundle GROUP BY region ORDER BY total_revenue DESC"
type: bar
title: Revenue by Region
```

## Top 10 Customers

```bundlebase
bundle: sales
query: "SELECT name, SUM(total) as lifetime_value FROM bundle GROUP BY name ORDER BY lifetime_value DESC LIMIT 10"
type: table
title: Top Customers by Lifetime Value
```

## Order Distribution

```bundlebase
bundle: sales
query: "SELECT region, COUNT(*) as order_count FROM bundle GROUP BY region"
type: pie
title: Orders by Region
```
"""
)
```

**Key points:**
- Each `bundlebase` code block needs: `bundle` (identifier of an open bundle), `query` (SQL), and `type` (table or chart type)
- Add an optional `title` for labeling
- The bundle must be open via MCP before generating the report
- You can reference multiple different bundles in the same report
- Surround data blocks with regular markdown for headings, commentary, and structure

**Reproducibility section:** Every report MUST end with a "Data Sources" section listing all bundles referenced in the report. Before generating the report, query each bundle's details with `SHOW DETAILS` and `SELECT * FROM bundle_info.connectors` to gather this information. Include for each bundle:
- **Path** (relative path or URL used to open it)
- **Name** and **version**
- **Sources** — describe each data source (connector type, URL/dataset, and what it provides)

Example ending for a report:

```markdown
---

## Data Sources

**sales** (`./sales-data`, version a3f8c1b2)
Sales transaction records from the CRM system.
- *http connector:* `https://api.example.com/sales.csv` — monthly sales export
- *kaggle connector:* `acme/product-catalog` — product reference data

**regions** (`./regions`, version 7e2d4f91)
Geographic region definitions and population data.
- *http connector:* `https://data.census.gov/regions.csv` — Census Bureau region boundaries
```

**CLI equivalent:**

```bash
bundlebase generate-report --input report.md --output report.pdf --bundle ./sales-data
```

**Storing reports for re-use:** Reports that a user would want to re-run over time as data changes can be stored in the bundle with `CREATE REPORT`. This keeps the report template alongside the data so anyone who opens the bundle can generate an up-to-date PDF at any time.

```
# Store the report in the bundle
mcp__bundlebase__query(
  bundle="sales",
  sql="""CREATE REPORT quarterly-sales
    NAME 'Quarterly Sales Report'
    DESCRIPTION 'Revenue breakdown by region and product'
    CONTENT $$
# Q4 Sales Report

## Revenue by Region

```bundlebase
bundle: sales
query: SELECT region, SUM(revenue) as total FROM bundle GROUP BY region ORDER BY total DESC
type: bar
title: Revenue by Region
```
$$"""
)

# Later, generate the PDF with current data
mcp__bundlebase__query(bundle="sales", sql="GENERATE REPORT quarterly-sales")
```

Other stored report commands:
- `SHOW REPORTS` — list all stored reports (id, name, description)
- `DROP REPORT <id>` — remove a stored report
- `SELECT * FROM bundle_info.reports` — query full report data including content

## Fetching External Data with Connectors

Bundlebase has built-in connectors for common data sources. The pattern is: CREATE SOURCE (auto-fetches on creation) → query/transform. You only FETCH again later to pick up new data.

**Do NOT use curl, wget, or requests to download data files.** Use bundlebase connectors instead — they handle downloading, format detection, versioning, and caching automatically.

**Choosing a connector:**

| Data source | Connector | Example |
|------------|-----------|---------|
| Any URL (CSV, TSV, JSON, Parquet) | `http` | `CREATE SOURCE USING http WITH (url = 'https://...')` |
| Kaggle dataset | `kaggle` | `CREATE SOURCE USING kaggle WITH (dataset = 'owner/name')` |
| S3/GCS/Azure/local directory | `remote_dir` | `CREATE SOURCE USING remote_dir WITH (url = 's3://...')` |
| FTP server | `ftp_directory` | `CREATE SOURCE USING ftp_directory WITH (url = 'ftp://...')` |
| SFTP server | `sftp_directory` | `CREATE SOURCE USING sftp_directory WITH (url = 'sftp://...')` |
| Webpage with file links | `web_scrape` | `CREATE SOURCE USING web_scrape WITH (url = 'https://...')` |
| PostgreSQL database | `postgres` | `CREATE SOURCE USING postgres WITH (url = 'postgres://...')` |
| Custom API with pagination/auth | Python connector | See "Building a Custom Connector" below |

**http connector `format` option:** When the URL doesn't have a file extension (e.g., API endpoints), the http connector auto-detects format from the Content-Type header. If that also fails, specify the format explicitly:

```sql
CREATE SOURCE USING http WITH (url = 'https://api.example.com/data?query=lakes', format = 'csv')
```

Valid formats: `csv`, `json`, `jsonl`, `parquet`, `tsv`. If omitted, format is auto-detected from Content-Type, then URL extension, then content inspection.

**http connector `head_supported` option:** The http connector makes a HEAD request to check the URL before downloading. Some servers (e.g., data.mn.gov, some government APIs) don't support HEAD requests properly. If you see an error mentioning HEAD request failure, retry with `head_supported = 'false'` to skip the HEAD check and go straight to downloading:

```sql
CREATE SOURCE USING http WITH (url = 'https://data.mn.gov/api/lake_quality.csv', head_supported = 'false')
```

**http connector POST/PUT and custom headers:** For APIs that require POST or PUT, use the `method` arg. Combine with `body` for a request body and `headers` for custom request headers (one `Name: Value` per line).

For JSON bodies, use dollar-quoting (`$$...$$`) so you don't need to escape double quotes:

```sql
-- POST with a JSON body (dollar-quoting avoids escaping issues)
CREATE SOURCE USING http WITH (
    url = 'https://api.example.com/data/query',
    method = 'POST',
    body = $${
        "filters": {"state": "MN", "type": "Lake"},
        "format": "csv"
    }$$,
    headers = 'Content-Type: application/json
Accept: text/csv'
)

-- POST with form-encoded body
CREATE SOURCE USING http WITH (
    url = 'https://api.example.com/data/query',
    method = 'POST',
    body = 'statecode=US%3A27&mimeType=csv',
    headers = 'Content-Type: application/x-www-form-urlencoded
Accept: text/csv'
)

-- GET with custom headers (auth token, requested format)
CREATE SOURCE USING http WITH (
    url = 'https://api.example.com/export',
    headers = 'Accept: text/csv
Authorization: Bearer my-api-token'
)
```

POST/PUT requests skip the HEAD probe entirely. `SAVE AS` defaults to AUTO as usual.

**`SAVE AS` clause (all sources):** Controls how fetched data is stored. **Omit it in most cases** — the default (`AUTO`) works correctly for all common formats. Only add `SAVE AS` when you have a specific reason.

| Value | Behavior |
|-------|----------|
| `AUTO` (default) | Reference attachable formats by URL when possible; copy when required by the connector; convert non-attachable formats (Excel) to Parquet |
| `COPY` | Download and copy original bytes into the bundle. Only valid for attachable formats (CSV, TSV, JSON, Parquet) |
| `PARQUET` | Convert everything to Parquet before storing |
| `REF` | Reference the remote URL directly — no download. Only valid for attachable formats and connectors that support it |

```sql
-- Recommended: omit SAVE AS — AUTO default handles CSV, JSON, Parquet, Excel correctly
CREATE SOURCE USING http WITH (url = 'https://example.com/data.csv')

-- Only add SAVE AS if you need a specific behavior:
-- Force download into the bundle (self-contained, works offline)
CREATE SOURCE USING http WITH (url = 'https://example.com/data.csv') SAVE AS COPY

-- Force Parquet conversion for all data
CREATE SOURCE USING http WITH (url = 'https://example.com/data.csv') SAVE AS PARQUET

-- Merge many small files into larger parquet batches
CREATE SOURCE USING remote_dir WITH (url = 's3://bucket/data/', patterns = '**/*.jsonl') MIN BATCH 15M

-- Excel files are automatically converted to Parquet even without SAVE AS
CREATE SOURCE USING http WITH (url = 'https://example.com/data.xlsx')
```

```bash
# URL: download CSV/TSV/JSON/Parquet from any HTTP(S) URL
bundlebase create --bundle ./lake-data "CREATE SOURCE USING http WITH (url = 'https://data.mn.gov/api/lake_quality.csv')"

# Kaggle: fetch a public dataset (requires ~/.kaggle/kaggle.json)
bundlebase create --bundle ./housing "CREATE SOURCE USING kaggle WITH (dataset = 'zillow/zecon', patterns = '*.csv')"

# S3: attach all parquet files from a bucket
bundlebase create --bundle ./logs "CREATE SOURCE USING remote_dir WITH (url = 's3://my-bucket/data/', patterns = '**/*.parquet')"

# Preview what would be fetched without actually fetching
bundlebase query --bundle ./logs --format json "FETCH base ADD DRY RUN"
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

When data lives behind a custom API or needs custom fetch logic, write a connector and follow this workflow:

**Step 1: Write the connector**

```python
# my_connector.py
from bundlebase_sdk import Connector, Location, serve

class MyApiConnector(Connector):
    def discover(self, attached_locations, **kwargs):
        return [Location("users.parquet", format="parquet", version="v1")]

    def data(self, location, **kwargs):
        import pyarrow as pa
        # ... call your API here ...
        return pa.table({"id": [1, 2], "name": ["Alice", "Bob"]})

if __name__ == "__main__":
    serve(MyApiConnector())
```

**Step 2: Test the connector with `TEST CONNECTOR`**

Validate that bundlebase can communicate with it before creating a source. `TEST CONNECTOR` calls discover() and data() and shows the schema and sample data without modifying the bundle.

```
# Test inline without importing (quick validation during development)
mcp__bundlebase__query(bundle="data", sql="TEST TEMP CONNECTOR 'python::my_connector.py:MyApiConnector'")

# Or test an already-imported connector
mcp__bundlebase__query(bundle="data", sql="IMPORT TEMP CONNECTOR my.api FROM 'python::my_connector.py:MyApiConnector'")
mcp__bundlebase__query(bundle="data", sql="TEST CONNECTOR my.api")
```

If the test fails, fix the connector and test again. Do not proceed to creating a source until the test passes.

**Step 3: Create a source — data is fetched automatically**

Once the test passes, create the source. `CREATE SOURCE` automatically discovers and fetches the data — there is no separate FETCH step needed.

```
mcp__bundlebase__query(bundle="data", sql="IMPORT TEMP CONNECTOR my.api FROM 'python::my_connector.py:MyApiConnector'")
mcp__bundlebase__query(bundle="data", sql="CREATE SOURCE USING my.api")
mcp__bundlebase__query(bundle="data", sql="COMMIT 'Loaded API data'")
```

**To refresh data later**, use `FETCH base ADD` (for new files) or `FETCH base SYNC` (add new + remove deleted):

```
mcp__bundlebase__query(bundle="data", sql="FETCH base SYNC")
mcp__bundlebase__query(bundle="data", sql="COMMIT 'Refreshed data'")
```

For persistent connectors (survive across sessions), use `IMPORT CONNECTOR` instead of `IMPORT TEMP CONNECTOR`. See the [Custom Connectors guide](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/index.md) and [Python SDK](https://raw.githubusercontent.com/nvoxland/bundlebase/main/docs/guide/custom-connectors/python.md).

## Transforming Data with Functions and Computed Columns

After attaching data, use computed columns and custom functions to clean and enrich it:

```bash
# Add computed columns using SQL expressions
bundlebase extend --bundle ./data "ADD COLUMN full_name AS first_name || ' ' || last_name"
bundlebase extend --bundle ./data "ADD COLUMN price_cents AS CAST(price * 100 AS INTEGER)"

# Cast column types
bundlebase extend --bundle ./data "CAST COLUMN price TO integer"

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

Use `EXPORT DATA TO` to save query results directly to a file. This is more efficient than piping query output through stdout, especially for large result sets — use it instead of `SELECT` when you need the data in a file.

```bash
# Export to CSV
bundlebase query --bundle ./analysis "EXPORT DATA TO 'output.csv' SELECT * FROM bundle"

# Export filtered results to JSON Lines
bundlebase query --bundle ./analysis "EXPORT DATA TO 'active_users.jsonl' SELECT * FROM bundle WHERE active = true"

# Export aggregated results
bundlebase query --bundle ./analysis "EXPORT DATA TO 'summary.csv' SELECT department, COUNT(*) as cnt, AVG(salary) as avg_sal FROM bundle GROUP BY department"
```

**Supported formats:** `.csv`, `.jsonl` (JSON Lines — one JSON object per line)

**Tip:** For data exploration where you need to see results, use `SELECT` with `--format json`. For saving data to a file for further processing, prefer `EXPORT DATA TO` — it streams directly to the file without row limits.

## Exporting Hollow Bundles

Use `EXPORT HOLLOW TO` to share a bundle's structure without its data. Recipients can open the hollow bundle and run `FETCH` to pull the raw data themselves.

```bash
# Export hollow bundle (no data, just sources and transformations)
bundlebase build --bundle ./analysis "EXPORT HOLLOW TO './shared/hollow'"

# Export as a portable tar file
bundlebase build --bundle ./analysis "EXPORT HOLLOW TO './shared/hollow.tar'"
```

A hollow bundle contains sources, always-update/always-delete rules, column renames/casts, joins, and views — but no attached data files. The `EXPECTED SCHEMA` is preserved so column operations resolve correctly before any data is fetched.

## Multi-platform Custom Connectors

When you build a custom (`ffi::`) connector for several OS/arch targets, register all binaries in one `IMPORT CONNECTOR` statement so the bundle ships every platform. The right binary is selected automatically at fetch time.

```sql
-- Explicit map: one entry per platform
IMPORT CONNECTOR acme.weather FROM {
    'linux/amd64'   : 'ffi::./weather-linux-amd64.so',
    'linux/arm64'   : 'ffi::./weather-linux-arm64.so',
    'darwin/arm64'  : 'ffi::./weather-darwin-arm64.dylib',
    'windows/amd64' : 'ffi::./weather-windows-amd64.dll'
};

-- Glob form: scan a directory for matching files
IMPORT CONNECTOR acme.weather FROM 'ffi::./weather-{os}-{arch}.{ext}';
```

The host-platform binary is verified by `dlopen`; foreign-platform binaries are checked structurally (ELF / Mach-O / PE magic + arch byte) since the build host can't load them.

### Bundling and Recovering Connector Source

Add `WITH (src = '/path/to/source.zip')` to ship the connector's source archive inside the bundle. The archive is content-addressed and travels with hollow exports:

```sql
IMPORT CONNECTOR acme.weather FROM 'ffi::./lib.so'
    WITH (platform = 'linux/amd64', src = './weather-source.zip');
```

Recipients of the bundle can extract the archive with `EXPORT SOURCE`:

```bash
bundlebase query --bundle ./bundle "EXPORT SOURCE acme.weather TO '/tmp/weather-source.zip'"
```

### Sharing Bundles

Bundles are portable — share them with teammates:

```bash
# Push a bundle to S3 so others can access it
bundlebase create --bundle s3://team-bucket/shared-analysis -m "Shared cleaned dataset" "ATTACH 'cleaned.parquet'"

# Others can then query it
bundlebase query --bundle s3://team-bucket/shared-analysis --format json "SHOW COLUMNS"
```

## SQL Reference Summary

The table name for bundle data is always `bundle`. Bundlebase uses **Apache DataFusion** as its SQL engine, so all standard DataFusion SQL syntax is supported. If you already know DataFusion's functions and syntax, use them directly. For reference, see the [DataFusion SQL documentation](https://datafusion.apache.org/user-guide/sql/index.html).

**Statistical and analytical functions** — DataFusion includes many built-in aggregate and statistical functions. Do NOT fall back to Python/pandas for analysis that SQL can handle. Key functions include:

| Function | Description | Example |
|----------|-------------|---------|
| `CORR(x, y)` | Pearson correlation coefficient | `SELECT CORR(price, sqft) FROM bundle` |
| `COVAR_POP(x, y)` | Population covariance | `SELECT COVAR_POP(x, y) FROM bundle` |
| `COVAR_SAMP(x, y)` | Sample covariance | `SELECT COVAR_SAMP(x, y) FROM bundle` |
| `STDDEV(x)` / `STDDEV_SAMP(x)` | Sample standard deviation | `SELECT STDDEV(price) FROM bundle` |
| `STDDEV_POP(x)` | Population standard deviation | `SELECT STDDEV_POP(price) FROM bundle` |
| `VAR_POP(x)` / `VAR_SAMP(x)` | Variance | `SELECT VAR_POP(score) FROM bundle` |
| `MEDIAN(x)` | Median value | `SELECT MEDIAN(price) FROM bundle` |
| `APPROX_PERCENTILE_CONT(x, p)` | Approximate percentile | `SELECT APPROX_PERCENTILE_CONT(price, 0.95) FROM bundle` |
| `REGR_SLOPE(y, x)` | Linear regression slope | `SELECT REGR_SLOPE(price, sqft) FROM bundle` |
| `REGR_INTERCEPT(y, x)` | Linear regression intercept | `SELECT REGR_INTERCEPT(price, sqft) FROM bundle` |
| `REGR_R2(y, x)` | R-squared | `SELECT REGR_R2(price, sqft) FROM bundle` |

**Window functions** are also supported for ranking, running totals, and moving averages:

```sql
SELECT name, revenue,
  RANK() OVER (ORDER BY revenue DESC) as rank,
  SUM(revenue) OVER (ORDER BY date ROWS BETWEEN 6 PRECEDING AND CURRENT ROW) as rolling_7day
FROM bundle
```

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
| `UPDATE bundle SET col = expr WHERE ...` | Update rows matching a condition |
| `DELETE FROM bundle WHERE ...` | Delete rows matching a condition |
| `ALWAYS DELETE FROM bundle WHERE ...` | Persistent rule: auto-delete matching rows on every future ATTACH |
| `DROP ALWAYS DELETE [WHERE ...]` | Remove one or all always-delete rules |
| `SHOW ALWAYS DELETES` | List active always-delete rules |
| `DESCRIBE DATA IN col1, col2` | Profile columns (min/max/avg/nulls/top values) |
| `DESCRIBE DATA IN col AS TYPE` | Detect sentinel values that fail to cast |

These SQL commands also work with `bundlebase query`, e.g. `bundlebase query --bundle ./data --format json "SHOW COLUMNS"`.
