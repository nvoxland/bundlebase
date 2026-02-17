# CLI REPL

The Bundlebase CLI includes an interactive REPL (Read-Eval-Print Loop) for working with bundles from the command line.

## Starting the REPL

```bash
bundlebase --bundle <path> [options]
```

### Flags

| Flag | Default | Description |
|---|---|---|
| `--bundle <path>` | *(required)* | Path or URL to the bundle |
| `--create` | `false` | Create a new bundle if it doesn't exist |
| `--read-only` | `false` | Open in read-only mode (only SELECT and EXPLAIN allowed) |
| `--log-level <level>` | `ui` | Logging level: `ui`, `trace`, `debug`, `info`, `warn`, `error` |
| `--otel <endpoint>` | *(none)* | OpenTelemetry endpoint for tracing |

### Examples

```bash
# Create a new bundle
bundlebase --bundle ./my-bundle --create

# Open an existing bundle
bundlebase --bundle ./my-bundle

# Open in read-only mode
bundlebase --bundle ./my-bundle --read-only

# Open with debug logging
bundlebase --bundle ./my-bundle --log-level debug

# Open a remote bundle
bundlebase --bundle s3://mybucket/my-bundle
```

!!! note
    The `--create` and `--read-only` flags cannot be combined.

## REPL Features

- **Command history** — Previous commands are saved to `~/.bundlebase/history.txt` (up to 1,000 entries) and recalled with the up/down arrow keys
- **Tab completion** — Press Tab to complete command names and column names
- **Emacs keybindings** — Standard Emacs shortcuts (Ctrl+A, Ctrl+E, Ctrl+K, etc.)
- **Exit** — Press `Ctrl+C`, `Ctrl+D`, or type `/exit`

## Meta-Commands

Meta-commands start with `/` and provide quick access to common operations:

| Command | Description |
|---|---|
| `/help` | Show available commands |
| `/show [limit <n>]` | Display bundle rows |
| `/schema` | Show column names and types |
| `/count` | Show total row count |
| `/status` | Show uncommitted changes |
| `/history` | Show commit history |
| `/details` | Show bundle metadata (id, name, URL, version) |
| `/clear` | Clear the terminal |
| `/exit` (`/quit`) | Exit the REPL |

Meta-commands are case-insensitive: `/SHOW`, `/Show`, and `/show` all work.

## SQL Commands

Any input that doesn't start with `/` is treated as SQL and executed against the bundle. This includes both standard SQL and Bundlebase-specific commands:

```
./my-bundle> ATTACH 'sales.parquet'
./my-bundle> FILTER WITH SELECT * FROM bundle WHERE revenue > 1000
./my-bundle> RENAME COLUMN fname TO first_name
./my-bundle> SELECT count(*) FROM bundle
./my-bundle> COMMIT 'Cleaned up sales data'
```

See the [SQL Reference](../sql-reference/index.md) for the full command syntax.

!!! note
    If you type a bare command name like `help` or `show` without the `/` prefix, the REPL will suggest the correct form (e.g., "Did you mean '/help'?").

## Output Formatting

Query results display as formatted tables. By default, output is limited to 100 rows. Use `/show limit <n>` to control the number of rows displayed:

```
./my-bundle> /show limit 10
```

For SQL queries, the full result set is displayed (up to the 100-row display limit).

## Example Session

```
$ bundlebase --bundle ./demo --create

Bundlebase REPL
Type '/help' for available commands, '/exit' to quit
----------------------------------------------------------
Creating bundle at: ./demo
./demo> ATTACH 'customers.csv'
./demo> /schema
+-----------+-----------+
| column    | type      |
+-----------+-----------+
| id        | Int64     |
| name      | Utf8      |
| email     | Utf8      |
| country   | Utf8      |
+-----------+-----------+

./demo> /count
4821

./demo> SELECT country, count(*) as n FROM bundle GROUP BY country ORDER BY n DESC
+---------+------+
| country | n    |
+---------+------+
| US      | 2104 |
| UK      | 891  |
| DE      | 743  |
| ...     | ...  |
+---------+------+

./demo> FILTER WITH SELECT * FROM bundle WHERE country = 'US'
./demo> /count
2104

./demo> COMMIT 'US customers only'
./demo> /history
+-----+-------------------+-------------------+
| ver | message           | timestamp         |
+-----+-------------------+-------------------+
| 1   | US customers only | 2026-01-30 12:00Z |
+-----+-------------------+-------------------+

./demo> /exit
Goodbye!
```
