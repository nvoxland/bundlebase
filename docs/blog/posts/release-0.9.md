---
date: 2026-04-15
categories:
  - Releases
---

# Bundlebase 0.9.0

Preparing for a beta release with stored reports, connector improvements, "hollow" bundles, and a solid round of performance work.

<!-- more -->

## New Features

### Stored reports

You can now define named queries and store them in the bundle with [`CREATE REPORT`](../../sql-reference/index.md#create-report). Run one with `GENERATE REPORT`, list them with `SHOW REPORTS`, and remove with `DROP REPORT`. Useful for packaging canned analyses alongside the data. See the [reports guide](../../guide/reports.md#stored-reports) for the full syntax.

### JSON normalization options

Three new connector options for flattening nested JSON into tabular form: `json_record_path` (dot-notation path to the record array), `json_sep` (separator used when flattening nested keys, defaults to `_`), and `json_meta` (outer fields to broadcast as columns on every row). JSON is converted to Parquet before attachment, same as Excel. Details in [Sources: JSON Options](../../guide/sources.md#json-options).

### CSV/TSV/JSONL → Parquet conversion

`source save as auto` now converts CSV, TSV, and JSONL files to Parquet at attach time, not just JSON and Excel. This means those formats get the same query performance as native Parquet.

If you want to keep the original data formats attached, use `SAVE AS REF` or `SAVE AS COPY`

### EXPORT DATA and EXPORT HOLLOW

The old `EXPORT` command is now [`EXPORT DATA`](../../sql-reference/index.md#export-data) for clarity. New: [`EXPORT HOLLOW`](../../sql-reference/index.md#export-hollow) exports the bundle structure and schema without the data — useful for sharing a template or moving a bundle definition without copying large files.

### HTTP connector POST/PUT

The [HTTP connector](../../guide/sources.md#http) now supports `POST` and `PUT` with request body and headers, not just `GET`. Useful for APIs that require authentication tokens or structured request bodies.

### `$$` syntax for inline data

You can now embed literal data directly in SQL using [dollar-quoted](../../sql-reference/index.md#string-literals) `$$...$$` blocks instead of referencing an external file. No escaping needed, which makes it practical for multi-line JSON bodies in things like HTTP connector requests.

## Performance

**COUNT(\*) is now instant.** Row counts are captured at attach time, so `SELECT COUNT(*) FROM bundle` never touches the underlying file. Previously it scanned the whole block.

**Narrow projections skip unnecessary work.** A query like `SELECT name FROM bundle` now pushes the column selection all the way into the CSV and JSONL row parsers — unused columns are never read or allocated. On wide schemas this makes a noticeable difference.

**Block and layout caches are larger by default.** The block cache default is now 500 MB (up from 64 MB), and the layout cache holds up to 5,000 entries. The layout cache now tracks evictions and duplicate inserts so you can see if the cache is undersized.

**Bundle open is faster.** Layout sidecars load in parallel at open instead of blocking, then lazily on demand after that.

**CSV attach is faster.** Row scanning uses memchr for newline detection and reuses the header schema across row groups.

**JSONL projection pushdown.** Column selection is pushed into the JSONL row parser, so queries on wide JSONL files only read the columns they need.

** Schemas and column IDs are now persisted as sidecar files alongside each block.** This makes reopening a bundle faster since the schema doesn't need to be re-inferred from the data file.

## Installation

**Python package:**

```
pip install bundlebase==0.9.0
```

**CLI binaries** (macOS arm64, Linux x86_64, Windows x86_64) are on the [releases page](https://github.com/nvoxland/bundlebase/releases) — no Python required. See the [install guide](../../getting-started/cli/install.md) for setup instructions.
