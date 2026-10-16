---
date: 2026-10-15
categories:
  - Releases
---

# Bundlebase 0.13.0

Functionality-wise, 0.13 adds a semantic model so a bundle can explain itself to an agent. The next release is going to focus on having that be more standards-compliant.

Internally, it reworks how DELETE and UPDATE are stored, plus fixes a bunch of security and correctness holes. A few defaults changed, so read the "Breaking changes" section before upgrading.

<!-- more -->


## Semantic model

- `DESCRIBE COLUMN amount AS 'Order total in USD, after discounts'` attaches a description to a column. Descriptions follow the column through renames.
- `CREATE METRIC revenue AS 'SUM(amount)' DESCRIPTION '...'` and `DROP METRIC revenue` define named calculations that live in the bundle. Metrics are validated when you define them and survive column renames.
- Both are regular operations, so they're versioned and undoable like everything else.
- Python gets `describe_column()`, `define_metric()`, `drop_metric()`, plus `column_descriptions()`, `metrics()`, `compute_metric()` and `data_quality()` to read them back.
- Flight exposes descriptions as field metadata on `GetTables`, so BI tools that read it will show them.

See the [semantic model guide](../../guide/semantic-model.md).

## MCP and agents

- New MCP tools: `describe` (everything about a bundle in one call), `analyze`, `data_quality`, and `metric`.
- `bundlebase create --exists-ok` opens the bundle if it already exists instead of erroring, which makes scripted agent workflows simpler.
- The docs have been reorganized around these use cases: [agent workspace](../../use-cases/agent-workspace.md), [scheduled collection](../../use-cases/scheduled-collection.md) and [sharing datasets](../../use-cases/sharing-datasets.md). The old persona pages are gone.

## DELETE and UPDATE

Every DELETE and UPDATE used to be stored twice: as row-level tombstones/overlays, and as a query-level filter replayed on every query. The UPDATE version of that filter stacked badly. Twenty UPDATEs on a 10K row bundle made each query take about a minute. Now tombstones and overlays are the only representation, so query time doesn't grow with the number of edits. Full details are in ADR-009.

Behavior changes that came with it:

- Edits apply beneath your `filter()` operations, so a filter defined earlier sees updated values.
- DELETE follows SQL NULL semantics: rows where the predicate is NULL are kept. Before, they were hidden from queries without actually being deleted.
- `ALWAYS DELETE` / `ALWAYS UPDATE` rules now only run against newly attached blocks on ATTACH. Before, they re-ran over everything, so `SET n = n + 1` incremented old rows again on every attach.
- Edits are tracked by block id instead of position, so DETACH no longer moves them onto the wrong block.
- UPDATE no longer counts or changes rows that were already deleted.
- `search()` sees UPDATEd values, including on the indexed column.
- Known gap: a block with a committed UPDATE skips the index fast path and does a full scan. An overlay-aware fast path is on the list.

## Security

- Untrusted bundle data can't run code by default. The trust model is written up in ADR-010. Short version: the person running bundlebase is trusted, the bundle's contents aren't.
- Credentialed http requests stop at cross-origin redirects instead of forwarding your auth header.
- `IMPORT CONNECTOR` no longer starts the connector's process to check it unless external code is allowed.
- Flight compares credentials in constant time, rejects expired tokens instead of quietly making a new session, and no longer logs tokens or SQL at info level.
- Index files and tar archives check sizes before trusting them, and tar writes are atomic.

## Data and performance

- Attaching, fetching and scanning CSV/JSONL now stream end to end instead of loading files into memory. One-shot `bundlebase query` output streams too.
- Several CSV/JSONL row-counting bugs are fixed, including quoted newlines, whitespace-only lines, and an exact `COUNT(*)` that could be wrong.
- TSV files can actually be attached now. They were missing from the reader list.
- Bundled connector and function code works from S3, GCS, Azure, tar and in-memory bundles, not just local ones.
- Parquet reads share DataFusion's metadata cache.
- `system.git_versioning`: attached files in a git working tree use their git blob id for change detection instead of mtime. See [git integration](../../guide/git-integration.md).
- `VERIFY DATA` works on read-only bundles.
- Upgraded to DataFusion 55, Arrow 59 and PyO3 0.29.

## Python

- The sync API's `extend`, `drop_function`, `status` and `verify_data` work now.
- Six builder methods that couldn't be chained now can.
- The type stubs match the runtime. There's a test for that now.
- A note on memory: `to_pandas()` / `to_polars()` stream batches but hold the whole result before building the DataFrame.  Use `stream_batches()` for results bigger than RAM.

Plus a long tail of smaller fixes from the audit: UTF-8 truncation panics, escaping bugs in generated SQL and Typst reports, IPC/FFI result validation, and more.

## Breaking changes

- **Ordering comparisons between text and numbers now error.** CSV, TSV and JSONL columns import as text, so `WHERE amount > 100` used to compare strings and quietly return the wrong rows. That's now an error with a hint to cast. It applies to queries, `filter()`, views, `ADD COLUMN`, and DELETE/UPDATE `WHERE` clauses. Equality comparisons still work. Fix it with `CAST COLUMN` or an inline `CAST(amount AS DOUBLE) > 100`.
- **External code is off by default for functions too.** `ffi`, `ipc`, `java`, `docker` and file-backed `python` functions now need `system.allow_external_code` set to `true`, same as connectors already did. Module-backed python is unaffected. The setting is checked at call time, so `SET CONFIG allow_external_code = 'true' FOR 'system'` takes effect right away. See [Functions](../../guide/functions.md#external-code-is-off-by-default).
- **Credentials no longer go in the manifest.** A postgres `url` with a password in it, or http `headers` with `Authorization`/`Cookie`, are now rejected. Use the runtime-only config keys instead: `postgres.password`, `http.auth`, and `http.secret_headers`. They're masked in `SHOW CONFIG`.
- **`bundlebase serve` binds to `127.0.0.1` by default** instead of `0.0.0.0`. Pass `--host` to expose it. Credentials come from `BUNDLEBASE_FLIGHT_USERNAME` / `BUNDLEBASE_FLIGHT_PASSWORD`. It still falls back to `admin`/`password` with a warning if they're unset, so set them if you bind wide.
- **Malformed JSONL lines error** during a scan instead of being skipped.
- **The DELETE tombstone format changed.** Bundles with DELETEs or UPDATEs committed by 0.12 can't be read by 0.13. I decided not to write a migration given how few people are using this yet. If that bites you, let me know.
- 
## Installation

**Python package:**

```
pip install bundlebase==0.13.0
```

**CLI binaries** (macOS arm64, Linux x86_64, Windows x86_64) are on the
[releases page](https://github.com/nvoxland/bundlebase/releases).
