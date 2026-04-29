---
date: 2026-04-29
categories:
  - Releases
---

# Bundlebase 0.10.0

Mostly bug fixes and rough edges from beta — auto-reindexing after ATTACH/FETCH, unified BM25 across multi-block indexes, a much friendlier REPL, and a new end-to-end example bundle for Claude Code transcripts.

<!-- more -->

## New Features

### Indexes auto-refresh after ATTACH and FETCH

This was the biggest sharp edge in 0.9. If you defined a text or btree index, then later attached or fetched data, the index didn't cover the new blocks until you remembered to run `REINDEX`. The shipped claude-history bundle hit this exactly — `search()` came back with zero rows after `FETCH base ADD`.

Now any command that attaches or replaces blocks (`ATTACH`, `REPLACE`, `FETCH`, `JOIN`, `CREATE SOURCE … WITH fetch=true`) runs the reindex in the same change. If you're bulk-loading and want to defer it, the new `NO INDEX` clause on `ATTACH` and `FETCH` opts out, and you run `REINDEX` yourself when ready. See the [indexing guide](../../guide/indexing.md) for the new behavior.

### `search()` table function

Full-text search now goes through a `search()` table function instead of `WHERE col MATCH ...`:

```sql
SELECT _score, * FROM search('web_search') ORDER BY _score DESC;
SELECT _score, * FROM search('search_text_idx', 'pg_dump AND timeout');
```

Single-arg form auto-resolves the index when there's only one. The two-arg form lets you name a specific index and use BM25 query syntax. `_score` is exposed as a real column you can sort, filter, and project.

Underneath: BM25 is now unified across all `IndexedBlocks` entries in the bundle, so the same row gets the same score regardless of how attaches happened to chunk into separate index passes. Pre-fix, scores diverged ~5% across runs depending on commit history.

### Claude Code transcript example bundle

A full end-to-end example: a Go FFI connector that walks `~/.claude/projects` JSONL files, registered into a structure-only bundle that anyone can `FETCH` against to load their own transcript history. The bundle ships indexes (project_id, session_id, event_type, timestamp, plus an inverted text index on `search_text`), two views, and the connector source as an attached zip so recipients can rebuild it. See the [Claude history example](../../examples/claude-history.md).

### IMPORT CONNECTOR multi-platform + bundled source

`IMPORT CONNECTOR` now accepts a platform map or a glob with `{os}/{arch}/{ext}` placeholders so a single bundle can ship native binaries for every platform you care about. An optional `WITH (src = '...')` attribute attaches a zip of the connector source so recipients can audit, fork, or rebuild. New `EXPORT SOURCE` command pulls that source archive back out.

```sql
IMPORT CONNECTOR my.conn FROM 'connectors/{os}/{arch}/conn.{ext}'
    WITH (src = 'connectors/src.zip');
```

### EXPORT EMPTY (renamed from EXPORT HOLLOW)

The `EXPORT HOLLOW` command — which strips data ops and keeps structure — is now `EXPORT EMPTY`. Same behavior, less awkward name.

### `CREATE SOURCE … FETCH / NO FETCH`

`CREATE SOURCE` auto-fetches by default. The new `NO FETCH` clause skips that — useful when defining a source you only want recipients to fetch on their own machine, like the claude-history example. Python: `create_source(..., fetch=False)`.

### `export_tar(gzip=True)`

`export_tar` (Rust + Python) takes an optional `gzip` flag, defaults to false. Use it when you want to ship a single compressed file.

### `FETCH` verbose and dry-run

`FETCH ... DRY RUN` now reports expected rows added/modified/removed (the connector declares `num_rows` per discovered location; nullable to distinguish "unknown" from `0`). `FETCH ... VERBOSE` emits one row per add/modify/remove action so you can see which source locations a sync touches before committing.

The summary schema is also reshaped: `pack | connector | source_id | source_locations_added | source_locations_modified | source_locations_removed | rows_before | rows_after`. The old `source_url` column is gone (it was always empty in practice).

### `SHOW BLOCKS` shows all source entries

When `MIN BATCH` merges many small files into one block, the block carries a list of `BatchedSource` entries — locations and versions per merged file. The old `bundle_info.blocks` schema only surfaced the first entry. New schema replaces `source_location` / `source_version` with `source_count` plus parallel `List<Utf8>` columns `source_locations` / `source_versions` so you can `unnest` or filter the full list.

## REPL Improvements

The REPL was the worst place to actually use bundlebase in 0.9. A few things have changed:

- **Multi-line input.** Pressing Enter without a trailing `;` drops to a continuation line. The grammar handles `;` inside quoted strings and `$$...$$` blocks correctly, so a stray semicolon in a literal won't terminate early. Slash commands stay single-line.
- **Ctrl-C while a query runs cancels the query** instead of killing the CLI. Prints `<Cancelling Query...>` so you know it landed. Ctrl-C at the prompt clears the buffer; a second consecutive Ctrl-C exits.
- **Query timing.** Each result is followed by `(123 ms)` / `(1.45 s)` / `(2m 30.1s)` so you can eyeball performance without an external clock.
- **Empty result sets print `(0 rows)`** instead of nothing.
- **Better parse errors** include a hint with the command's usage when a known command is malformed (e.g. bare `COMMIT` now points at the missing message).

## Bug Fixes

- **`COUNT(*)` no longer reads block files.** DataFusion was calling `scan(projection=None)` for the pre-optimisation count plan and the fast path only matched `Some(empty)` — the `None` case fell through and eagerly populated the block cache. ~600 ms on a 5-block bundle for what should be metadata-only.
- **`search()` no longer returns 0 hits after a multi-block fetch.** Each `IndexedBlocks` entry was assigning local block-ref values 0..N over its own block list, so two entries' refs collided in the unified score map and hits silently overwrote each other. Block refs are now unified across entries at scan time.
- **`search()` works after `BundleBuilder::extend()`.** The UDTF held a weak ref to the original `Bundle` Arc, which `extend()` dropped — calls errored with "Bundle has been dropped". Re-registered against the builder facade.
- **`UPDATE`-then-`COMMIT`-then-reopen now persists.** `BundleBuilder::operations()` was returning `bundle.operations + status.operations()`, but `apply_operation` already pushed eagerly to `bundle.operations` — every uncommitted op was double-counted. Index builders ran on each block twice. The status union is gone.
- **DELETE persists across reopen.** The end-to-end persistence path was already in place but had no test; two new e2e tests now cover the full DELETE → commit → reopen → tombstone-applied chain.
- **`create_index` no longer fails on `Utf8` vs `Utf8View` block schemas.** Type widening + per-block casting handles mixed string types across blocks. `IndexedValue` now supports `Utf8View` directly.
- **Schema sidecars are pruned with their indexes.** `IndexDefinition::prune_stale_blocks` had zero callers — long-lived bundles leaked `Arc<IndexedBlocks>` proportional to commit history. Wired into the change-finalize path.
- **`Cargo.lock` is now tracked.** CI was breaking against transitive RC dependencies that resolved differently in fresh registries. Standard practice for binary-shipping projects.

## Internals

- **UDFs are normalized to stable `fn_<id>` names** internally, mirroring the `col_<id>` pattern for columns. Renaming a function is now a metadata-only operation — DataFusion's registration survives. SQL fragments are rewritten at command-build time so user-visible names work identically.
- The build script `docs/examples/scripts/rebuild_all.py` discovers every example dataset under `docs/examples/scripts/`, nukes prior outputs, and rebuilds. One example today (claude_history); convention-driven so future examples drop in.

## Installation

**Python package:**

```
pip install bundlebase==0.10.0
```

**CLI binaries** (macOS arm64, Linux x86_64, Windows x86_64) are on the [releases page](https://github.com/nvoxland/bundlebase/releases) — no Python required. See the [install guide](../../getting-started/cli/install.md) for setup instructions.

Let me know if you run into anything broken — the changes to auto-reindex and unified BM25 in particular touched a lot of code paths.
