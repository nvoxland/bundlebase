---
title: CLI Quick Start -- Bundlebase
description: Get started with the Bundlebase CLI. Create a bundle, attach data, and query it interactively from the REPL.
---

# CLI Quick Start

## Create a bundle and open the REPL

```bash
bundlebase repl --bundle ./my-bundle --create
```

You'll see a header and prompt:

```
Opened bundle at file://tmp/my-bundle (version empty, 1 commit)
Type '/help' for available commands, '/exit' to quit
----------------------------------------------------------
my-bundle>
```

## Attach data

```
my-bundle> ATTACH 'customers.csv'
```

## Query

```
my-bundle> SELECT * FROM bundle
```

## Filter rows

```
my-bundle> FILTER WITH SELECT * FROM bundle WHERE country = 'US'
```

## Commit

```
my-bundle> COMMIT 'Added and filtered customer data'
```

## View history

```
my-bundle> SHOW HISTORY
```

## Exit

Type `/exit`, `/quit`, or press `Ctrl+C` / `Ctrl+D`.

## Next steps

- [CLI REPL Guide](../../guide/cli-repl.md) -- full REPL reference
- [SQL Server Guide](../../guide/cli-serve.md) -- remote access from Metabase, R, DBeaver
- [Basic Concepts](../basic-concepts.md) -- bundles, operations, and versioning
