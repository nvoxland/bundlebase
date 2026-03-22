# MCP Server

The Bundlebase CLI can run as a [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server over stdio, allowing AI assistants like Claude Code, Cursor, and Copilot to interact with bundles directly through tool calls.

Unlike the CLI `query` command which opens and closes the bundle on every invocation, MCP mode keeps the bundle open for the lifetime of the server process, preserving cache and state between calls.

## Starting the Server

```bash
bundlebase mcp --bundle <path> [options]
```

### Flags

| Flag | Default | Description |
|---|---|---|
| `--bundle <path>` | *(required)* | Path or URL to the bundle |
| `--read-only` | `false` | Only allow SELECT and EXPLAIN commands |
| `--config <path>` | *(none)* | Path to a YAML/JSON config file |
| `--log-level <level>` | `ui` | Logging level |

### Examples

```bash
# Open an existing bundle as MCP server
bundlebase mcp --bundle ./my-bundle

# Read-only access
bundlebase mcp --bundle ./my-bundle --read-only

# Remote bundle
bundlebase mcp --bundle s3://mybucket/my-bundle
```

To serve a new bundle, create it first with `bundlebase create`.

## Available Tools

The MCP server exposes the following tools to AI assistants:

| Tool | Parameters | Description |
|---|---|---|
| `query` | `sql` (string, required) | Execute any SQL query or bundlebase command |
| `schema` | *(none)* | Get column names, data types, and nullability |
| `count` | *(none)* | Get total row count |
| `sample` | `limit` (integer, optional, default 10) | Preview sample rows as JSON |
| `status` | *(none)* | Show uncommitted changes |
| `history` | *(none)* | Show commit history |

### The `query` Tool

The `query` tool is the primary tool, handling all SQL queries and bundlebase commands:

- **SELECT queries**: `SELECT * FROM bundle WHERE revenue > 1000`
- **Data modification**: `ATTACH 'data.csv'`, `DETACH 'old.csv'`, `REPLACE 'a.csv' WITH 'b.csv'`
- **Transformations**: `FILTER WITH SELECT * FROM bundle WHERE active = true`
- **Schema changes**: `RENAME COLUMN fname TO first_name`, `DROP COLUMN temp_id`
- **Version control**: `COMMIT 'Added sales data'`, `RESET`, `UNDO`
- **Introspection**: `SHOW HISTORY`, `SHOW STATUS`, `SHOW CONFIG`, `SHOW DETAILS`, etc.
- **Help**: `SYNTAX` to list all commands, `SYNTAX ATTACH` for detailed syntax and examples
- **Everything else**: `JOIN`, `CREATE VIEW`, `CREATE TEXT INDEX`, `EXPLAIN`, etc.

All query results are returned as JSON, limited to 1000 rows.

See the [SQL Reference](../sql-reference/index.md) for the full command syntax.

## Configuring AI Assistants

### Claude Code

Add to your Claude Code MCP settings (`.claude/settings.json` or project settings):

```json
{
  "mcpServers": {
    "bundlebase": {
      "command": "bundlebase",
      "args": ["mcp", "--bundle", "./my-bundle"]
    }
  }
}
```

### Cursor

Add to your Cursor MCP configuration (`.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "bundlebase": {
      "command": "bundlebase",
      "args": ["mcp", "--bundle", "./my-bundle"]
    }
  }
}
```

### VS Code (Copilot)

Add to your VS Code MCP settings (`.vscode/mcp.json`):

```json
{
  "servers": {
    "bundlebase": {
      "command": "bundlebase",
      "args": ["mcp", "--bundle", "./my-bundle"]
    }
  }
}
```

## When to Use MCP vs CLI

| Scenario | Recommended Mode |
|---|---|
| One-shot query or schema check | CLI (`bundlebase query`) |
| Single ATTACH or mutation | CLI (`bundlebase extend`) |
| Building a new bundle from multiple files | MCP |
| Iterative data exploration | MCP |
| Multi-step transformations | MCP |
| Building up joins and views | MCP |

**CLI mode** (`bundlebase query` for reads, `bundlebase extend` for mutations) opens and closes the bundle on every call. `extend` auto-commits after each command. Efficient for simple, standalone operations but has overhead for multi-step workflows.

**MCP mode** keeps the bundle open, so the cache is warm and state is preserved. Use it when you need multiple related operations in sequence.

## Read-Only Mode

When started with `--read-only`, the server restricts operations:

- `SELECT` and `EXPLAIN` queries execute normally
- The `schema`, `count`, `sample`, and `history` tools work normally
- All modification commands (`ATTACH`, `FILTER`, `COMMIT`, etc.) return an error
- Useful for giving AI assistants safe, read-only access to data
