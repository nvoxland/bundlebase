# CLI REPL

The Bundlebase CLI includes an interactive REPL (Read-Eval-Print Loop) for working with bundles from the command line.

## Starting the REPL

```bash
bundlebase repl --bundle <path> [options]
```

### Flags

| Flag | Default | Description |
|---|---|---|
| `--bundle <path>` | *(required)* | Path or URL to the bundle |
| `--read-only` | `false` | Open in read-only mode (only SELECT and EXPLAIN allowed) |
| `--format <format>` | `table` | Output format: `table` or `json` |
| `--log-level <level>` | `ui` | Logging level: `ui`, `trace`, `debug`, `info`, `warn`, `error` |
| `--otel <endpoint>` | *(none)* | OpenTelemetry endpoint for tracing |

### Examples

```bash
# Open an existing bundle
bundlebase repl --bundle ./my-bundle

# Open in read-only mode
bundlebase repl --bundle ./my-bundle --read-only

# Open with debug logging
bundlebase repl --bundle ./my-bundle --log-level debug

# Open a remote bundle
bundlebase repl --bundle s3://mybucket/my-bundle
```

To create a new bundle, use `bundlebase create` first:

```bash
bundlebase create --bundle ./my-bundle
bundlebase repl --bundle ./my-bundle
```

## Non-Interactive Execution

For one-shot operations without the interactive REPL:

- **`bundlebase query`** -- Read-only queries (SELECT, EXPLAIN, SHOW, SYNTAX, meta-commands)
- **`bundlebase create`** -- Create a new bundle, optionally with initial commands
- **`bundlebase extend`** -- Mutating commands (ATTACH, FILTER, DROP, etc.) with auto-commit
- **`bundlebase list-bundles`** -- Discover all bundles in a directory

```bash
# Read-only query
bundlebase query --bundle <path> "SELECT * FROM bundle"

# Mutating command (auto-commits afterward)
bundlebase extend --bundle <path> "ATTACH 'data.csv'"

# Custom commit message
bundlebase extend --bundle <path> -m "Added sales data" "ATTACH 'sales.csv'"

# Pipe SQL from stdin
echo "SELECT count(*) FROM bundle" | bundlebase query --bundle ./my-bundle
```

`bundlebase execute` is an alias for `bundlebase extend`. See `bundlebase query --help` and `bundlebase extend --help` for details.

## Multiple Statements

You can execute multiple statements in a single input by separating them with semicolons (`;`). This works in the REPL, `bundlebase query`, and `bundlebase extend`:

```bash
# Multiple statements in the REPL
./my-bundle> ATTACH 'data.csv'; RENAME COLUMN fname TO first_name; SHOW COLUMNS

# Multiple statements via extend (all committed together)
bundlebase extend --bundle ./data "ATTACH 'sales.csv'; FILTER WITH SELECT * FROM bundle WHERE amount > 0"

# Multiple queries
bundlebase query --bundle ./data "SHOW COLUMNS; SHOW COUNT"
```

All statements are **validated before any execute**: if any statement has a syntax error, none will run. Semicolons inside single-quoted strings are handled correctly (e.g., `COMMIT 'added; cleaned'` is treated as one statement).

When using `bundlebase extend` with multiple statements, all changes are **committed together** as a single commit after all statements complete.

## REPL Features

- **Command history** -- Previous commands are saved to `~/.bundlebase/history.txt` (up to 1,000 entries) and recalled with the up/down arrow keys
- **Tab completion** -- Press Tab to complete command names and column names
- **Emacs keybindings** -- Standard Emacs shortcuts (Ctrl+A, Ctrl+E, Ctrl+K, etc.)
- **Exit** -- Press `Ctrl+C`, `Ctrl+D`, or type `/exit`

## Meta-Commands

Meta-commands start with `/` and provide quick access to common operations:

| Command | Description |
|---|---|
| `/help` | Show available commands |
| `/clear` | Clear the terminal |
| `/exit` (`/quit`) | Exit the REPL |

For inspecting bundle data and metadata, use SQL commands:

| SQL Command | Description |
|---|---|
| `SELECT * FROM bundle` | Display bundle rows |
| `SHOW COLUMNS` | Show column names and types |
| `SHOW COUNT` | Show total row count |
| `SHOW STATUS` | Show uncommitted changes |
| `SHOW HISTORY` | Show commit history |
| `SHOW DETAILS` | Show bundle metadata (id, name, URL, version) |

## SQL Commands

Any input that doesn't start with `/` is treated as SQL and executed against the bundle. This includes both standard SQL and Bundlebase-specific commands like `SHOW` and `SYNTAX`:

```
./my-bundle> ATTACH 'sales.parquet'
./my-bundle> FILTER WITH SELECT * FROM bundle WHERE revenue > 1000
./my-bundle> RENAME COLUMN fname TO first_name; RENAME COLUMN lname TO last_name
./my-bundle> SELECT count(*) FROM bundle
./my-bundle> COMMIT 'Cleaned up sales data'
```

Use `SHOW` to inspect bundle metadata and `SYNTAX` to discover available commands:

```
./my-bundle> SHOW HISTORY
./my-bundle> SHOW STATUS
./my-bundle> SHOW CONFIG
./my-bundle> SYNTAX IMPORT FUNCTION
```

See the [SQL Reference](../sql-reference/index.md) for the full command syntax.

!!! note
    If you type a bare command name like `help` without the `/` prefix, the REPL will suggest the correct form (e.g., "Did you mean '/help'?").

## Output Formatting

Query results display as formatted tables. By default, output is limited to 100 rows. Use `LIMIT` in SQL queries to control the number of rows displayed:

```
./my-bundle> SELECT * FROM bundle LIMIT 10
```

## Example Session

```
$ bundlebase create --bundle ./demo "ATTACH 'customers.csv'"
$ bundlebase repl --bundle ./demo

Opened bundle at ./demo (version ..., 1 commit)
Type '/help' for available commands, '/exit' to quit
----------------------------------------------------------
./demo>
./demo> SHOW COLUMNS
+-----------+-----------+
| column    | type      |
+-----------+-----------+
| id        | Int64     |
| name      | Utf8      |
| email     | Utf8      |
| country   | Utf8      |
+-----------+-----------+

./demo> SHOW COUNT
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
./demo> SHOW COUNT
2104

./demo> COMMIT 'US customers only'
./demo> SHOW HISTORY
+-----+-------------------+-------------------+
| ver | message           | timestamp         |
+-----+-------------------+-------------------+
| 1   | US customers only | 2026-01-30 12:00Z |
+-----+-------------------+-------------------+

./demo> /exit
Goodbye!
```

## Listing Bundles

The `list-bundles` subcommand discovers all bundles in a directory and prints their name and description.

```bash
bundlebase list-bundles [path]
```

### Arguments

| Argument | Default | Description |
|---|---|---|
| `path` | `.` | Path or URL to search for bundles |

### Examples

```bash
# List bundles in the current directory
bundlebase list-bundles

# List bundles in a specific directory
bundlebase list-bundles --path /data/bundles

# List bundles in an S3 bucket
bundlebase list-bundles --path s3://my-bucket/bundles/
```

```
$ bundlebase list-bundles /data
customers : Customer Data
    US customer records from 2025

sales : Sales Bundle
    Monthly sales aggregates

inventory : (name not set)
    (description not set)
```

If a bundle has a name set, it appears after the directory name separated by ` : `. Descriptions are indented below. Bundles without a name or description show `(name not set)` or `(description not set)`.

## Agent Skills

The `setup-agent` subcommand installs local agent integration for Claude Code and GitHub Copilot.

### Project-Level Install (default)

```bash
bundlebase setup-agent
```

Auto-detects supported agents from your `PATH` (`claude` for Claude Code, `copilot` for Copilot) and installs into the current directory for whatever it finds.

Use `--agent claude` or `--agent copilot` to bypass auto-detection.

For Claude Code, it installs:

- `.agents/skills/bundlebase/SKILL.md` and `reference.md` -- agent skill files with CLI workflows and SQL command reference
- `.mcp.json` -- Claude Code MCP server config
- Appends a `## Bundlebase` section to `CLAUDE.md` (creates the file if it doesn't exist) so agents always know bundlebase is available

For GitHub Copilot, it installs:

- `.vscode/mcp.json` -- workspace MCP server config for Bundlebase
- `AGENTS.md` -- always-on Bundlebase guidance for Copilot chat/agent workflows

### User-Level Install

```bash
bundlebase setup-agent --scope global
```

Global scope is Claude-only. It installs to `~/.agents/skills/bundlebase/`, `~/.mcp.json`, and `~/CLAUDE.md`, making bundlebase skills available across all projects.

### Notes

- The command is idempotent: running it again refreshes Bundlebase-managed config and skill files
- No `--bundle` flag is needed
- If auto-detection finds nothing on `PATH`, rerun with `--agent claude` or `--agent copilot`
- The installed files contain CLI usage patterns and the full SQL command reference so agents can use bundlebase without making web requests to documentation
