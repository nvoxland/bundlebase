---
date: 2025-12-20
categories:
  - Releases
---

# Bundlebase 0.1.0 -- First Release

I pushed out the first actual release of Bundlebase today. CI's always a pain, so the first actually working version is 0.1.2.

It has the core of what Bundlebase does:

- Attach data from Parquet, CSV, and JSON files
- Filter, select, rename, and remove columns
- Query with SQL (via DataFusion)
- Commit snapshots with version history
- Row indexing for fast lookups
- Custom function system for generated data
- Full async Python API with a sync wrapper for notebooks

There's also a CLI with a REPL, though that's more of a debugging tool at this point.

```
pip install bundlebase==0.1.2
```

## Where things stand

This is not production-ready code. The indexing is largely untested in real use, the join support is rough, and I'm sure there are bugs I haven't found yet. But it runs, and you can install it with pip.

What exists today:

- Attach data from Parquet, CSV, JSON, or custom sources
- Filter, transform, rename, remove columns
- Query with SQL
- Commit snapshots with version history
- Full async Python API (plus a sync wrapper for notebooks)

What doesn't exist yet but I want to build:

- Publishing and pulling bundles from a registry
- Better tooling for inspecting bundle contents and history
- More data source adapters

