# CLI Quick Start

This guide walks through a basic workflow using the Bundlebase CLI's interactive REPL.

## Start the REPL

Create a new bundle and open the interactive REPL:

```bash
bundlebase create --bundle ./my-bundle
bundlebase repl --bundle ./my-bundle
```

You'll see a header and prompt:

```
Opened bundle at ./my-bundle (version ..., 1 commit)
Type '/help' for available commands, '/exit' to quit
----------------------------------------------------------
./my-bundle>
```

The prompt shows your bundle path on the left and the current time on the right.

## Attach Data

Type SQL commands directly at the prompt to work with your bundle:

```
./my-bundle> ATTACH 'customers.csv'
```

## Query Your Data

Use standard SQL to explore the data:

```
./my-bundle> SELECT * FROM bundle
```

## Filter Rows

Narrow down the data in your bundle:

```
./my-bundle> FILTER WITH SELECT * FROM bundle WHERE country = 'US'
```

## Commit Changes

Save your work as a new version:

```
./my-bundle> COMMIT 'Added and filtered customer data'
```

## View History

Check the commit log:

```
./my-bundle> SHOW HISTORY
```

## Discover Commands

Type `/help` to see all available REPL commands, or refer to the [SQL Reference](../sql-reference/index.md) for the full command syntax.

## Exit the REPL

Type `/exit`, `/quit`, or press `Ctrl+C` / `Ctrl+D`.

## Next Steps

- **[CLI REPL Guide](../guide/cli-repl.md)** — Full REPL reference with all flags and commands
- **[Flight SQL Server Guide](../guide/cli-flight.md)** — Remote access via Arrow Flight SQL
- **[Basic Concepts](basic-concepts.md)** — Bundles, operations, and versioning
