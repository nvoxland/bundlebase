---
date: 2026-05-01
categories:
  - Releases
---

# Bundlebase 0.11.0

Quality of life improvements, performance improvements, and bugfixes

<!-- more -->

## Big bugs fixed

- Full-text search returned 0 hits on real bundles 
- `bundle.view("name")` failed on fresh bundles
- bundlebase query` silently truncated to 1000 rows
- Flight server rejected vanilla PyArrow clients
- `SELECT COUNT(*) FROM search('term')` errored
- Information schema is enabled on every bundle's session, so `SHOW TABLES` etc. work natively.

## Performance

- Improved `search()` per-call assembly cost
- `COUNT(*)` no longer reads block files

## CLI / REPL

- Support Multi-line input in the REPL. NOTE: this now requires `;` to run the command.
- Ctrl-C cancels the running query instead of killing the CLI
- Query timing line after every result
- Empty result sets print `(0 rows)` instead of nothing
- `DESCRIBE bundle`, `SHOW TABLES`, `SHOW COLUMNS FROM …` all work in the CLI now
- Wide-row tables are bounded.
- `Int64(1)` headers in scalar selects normalize. `SELECT 1, 'hi'` now shows `1` and `"hi"` as headers instead of DataFusion's debug repr.
- Date / time / timestamp / decimal / interval columns format properly

## Python

- `SyncBundle` got the missing methods it should have had
- `SyncBundle.stream_batches()` is true incremental streaming

## Installation

**Python package:**

```
pip install bundlebase==0.11.0
```

**CLI binaries** (macOS arm64, Linux x86_64, Windows x86_64) are on the [releases page](https://github.com/nvoxland/bundlebase/releases).
