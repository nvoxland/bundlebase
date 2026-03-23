---
date: 2026-01-30
categories:
  - Releases
---

# Bundlebase 0.4.0

This release is about how you interact with bundles. You can now run any bundle modification through a SQL-like string instead of only through the python API methods.
Those SQL-like command strings can be passed to a new `bundle.execute()` method in the Python API.

I've also cleaned up the REPL to accept any of those SQL-like commands, and distinguished REPL-specific commands with a `/` like `/exit` and `status`.

Finally, there is also a new Arrow Flight server you can start with the CLI instead of REPL mode which lets you run both queries and other commands from your favorite database client.

<!-- more -->

## New Features

### Arrow Flight server

You can now serve a bundle over the network using Arrow Flight SQL. Any Flight SQL client can connect and run queries.

```bash
bundlebase-cli --bundle /data/my_bundle --mode flight --port 50051
```

The server supports the full range of bundlebase commands (ATTACH, FILTER, CREATE VIEW, etc.) in addition to regular SQL queries. There's also a `--read-only` flag if you just want to expose data without allowing mutations.

Authentication is required. It ships with default credentials of `admin/password` for now, and will support configuring that in the future.

### REPL slash commands

REPL meta-commands now use a `/` prefix to distinguish them from SQL. The old bare-word commands are gone.

- `/schema` -- show the table schema
- `/count` -- row count
- `/details` -- bundle metadata (id, name, url, version)
- `/history` -- commit history
- `/status` -- uncommitted changes
- `/show [limit <n>]` -- display rows
- `/help`, `/exit`, `/clear`

Everything else you type in the REPL is treated as SQL.


### Data verification

Added `verify_data()` for validating data integrity across operations:

```python
await bundle.verify_data()
```

### Bundle metadata

I crated a `bundle_info` schema for metadata views.

- `bundle_info.details` gives information about the bundle
- `bundle_info.status` lists uncommitted changes
- `bundle_info.history` lists commit history

## Breaking Changes

### `select` is now `query`

The `select()` command has been replaced with `query()`. The key difference: `query()` returns results directly instead of creating a new bundle. If you want to persist query results, use `extend` explicitly.

### REPL commands require `/` prefix

Meta-commands like `help` and `schema` now need a leading slash: `/help`, `SHOW COLUMNS`. The REPL will suggest the correct form if you forget.

### Schema naming

The default schema changed from `public` to `default`. And `bundle_history` moved to `bundle_info.history`.

```
pip install bundlebase==0.4.0
```

Let me know if you run into issues, especially with the Flight server.
