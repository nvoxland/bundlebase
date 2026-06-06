---
date: 2026-04-02
categories:
  - Releases
---

# Bundlebase 0.8.0

Two themes drove most of what landed in 0.8: mutable data and making Bundlebase work better as an agent tool. 

<!-- more -->

## New Features

### DELETE and UPDATE

Rows can now be [deleted](../../sql-reference/index.md#delete) or [updated](../../sql-reference/index.md#update). The updates and deletes happen as layers over the existing data rather than physically modifying it so the history stays intact.

```python
# Delete rows
bundle.delete("price < 0")

# Update rows
bundle.update({"status": "'closed'"}, "last_login < '2024-01-01'")
```

Or SQL:

```sql
DELETE WHERE price < 0
UPDATE SET status = 'closed' WHERE last_login < '2024-01-01'
```

The big goal with the update and delete support is help you clean your datasets. 


### ALWAYS DELETE and ALWAYS UPDATE

[Persistent mutation rules](../../sql-reference/index.md#always-delete) that apply automatically to every fetch — useful for filtering out bad data at the source rather than after every import.

```python
bundle.always_delete("is_test = true")
bundle.always_update({"region": "'unknown'"}, "region IS NULL")
```

```sql
SHOW ALWAYS DELETES
```

## Agentic Usage

A lot of small things in this release were aimed at making Bundlebase easier for an agent to explore and use without hand-holding.

The [MCP server](../../guide/cli-mcp.md) now supports multiple bundles in a single session, so an agent can work across datasets without restarting. `list-bundles` lets it discover what's available. [`SYNTAX <command>`](../../sql-reference/index.md#syntax) gives inline help on any command, and [`bundle_info.commands`](../../sql-reference/index.md#show) lists every supported SQL command — so an agent can figure out what's possible without needing external docs.

The [HTTP connector](../../guide/sources.md#http) means an agent can pull in data from a URL directly, with format auto-detected from content type. Combined with [`TEST CONNECTOR`](../../sql-reference/index.md#test-connector), it can validate a data source before committing anything.

The REPL's `/` meta-commands are replaced with proper `SHOW` commands that work the same way in both interactive and programmatic use — less special-casing needed.

You can now test a connector without importing it or creating a bundle first. Runs `discover()` then `data()` and returns the results as a stream.

MCP server now supports multiple bundles concurrently in one session.

## Data Exploration

Whether you are driving Bundlebase with an agent or own your own, I added new features to help explore and understand your data.

- [`DESCRIBE DATA IN`](../../sql-reference/index.md#describe-data) statement provides column profiling: min, max, null count, distinct count
- [PDF reports](../../guide/reports.md) — embed live charts and tables in a markdown file, generate a PDF
- [`SHOW COMMANDS`](../../sql-reference/index.md#show) — lists all supported SQL commands with `bundle_info.commands`
- [`SYNTAX <command>`](../../sql-reference/index.md#syntax) — inline help for any command
- `list-bundles` command

## Additional Data Formats

TSV has been added as an attachable file format

Excel and standard JSON files can be returned by connectors, and Bundlebase auto-converts them into a supported internal format.


## Standalone CLI Binaries

Each release now ships [binaries](../../getting-started/cli/install.md) for macOS (arm64), Linux (x86_64), and Windows (x86_64) — no Python required. Download from the [releases page](https://github.com/nvoxland/bundlebase/releases) and put on your PATH.

## Performance

A few things landed that make a noticeable difference on larger datasets:

- **LRU block cache** — decoded `RecordBatch` objects are cached between queries, giving around 13x speedup at 100K rows in benchmarks
- **Parquet filter pushdown** — predicate pushdown is now enabled, so filters get pushed into the parquet reader rather than scanning everything
- **ZSTD compression** — parquet files now use ZSTD instead of the default
- **Sorted-vec tombstones** — DELETE filtering is significantly cheaper for high-delete-count bundles

See the [performance guide](../../guide/performance.md) for details on tuning.

---

```
pip install bundlebase==0.8.0
```
