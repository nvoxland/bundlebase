---
date: 2026-01-17
categories:
  - Releases
---

# Bundlebase 0.3.0

v0.3.0 is out. The big theme this release is data ingestion -- pulling data into bundles from external systems.

## New Features

### Sources

Up until now, you had to attach local files to a bundle. That's fine for a demo, but the whole point is to work with data wherever it lives. This release adds a source system that lets you define where data comes from and fetch it on demand:

```python
await c.create_source("inventory",
    location="sftp://warehouse.example.com/exports/",
    pattern="*.parquet")
await c.fetch("inventory")
```

There are three built-in source functions so far:

- **Remote directory** Pull files from FTP, SFTP, or object storage (S3, GCS, Azure) by path and glob pattern
- **Web scraper** Fetch data from web pages
- **PostgreSQL** Query a Postgres database with automatic partitioning by sort column and batch size

Sources support sync modes (add-only, update, full sync) so you can re-fetch and only get what's changed. I also added `detach_block` and `replace_block` operations to manage the data lifecycle -- you can swap out stale data without losing history.

### Joins are first-class

Joins were kind of bolted on before -- you'd `attach_to_join` and hope for the best. Now they're proper managed entities with `create_join`, `drop_join`, and `rename_join`, consistent with how views work.

The old `attach_to_join` is gone. You just `attach` with a join target now, which is simpler.

### Reset and Undo

Added `reset` and `undo` operations in Python

## Breaking Changes

### API naming cleanup

I went through and standardized the operation names:

- `define_function` -> `create_function`
- `define_index` -> `create_index`
- `remove_column` -> `drop_column`
- `define_source` -> `create_source`
- `refresh` -> `fetch`

Also changed the SQL table name from `select * from data` to `select * from bundle`

```
pip install bundlebase==0.3.0
```
