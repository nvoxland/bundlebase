# MCP Server

The Bundlebase CLI can run as a [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server over stdio, allowing AI assistants like Claude Code, Cursor, and Copilot to interact with bundles directly through tool calls.

Unlike the CLI `query` command which opens and closes the bundle on every invocation, MCP mode keeps bundles open for the lifetime of the server process, preserving cache and state between calls. Multiple bundles can be open simultaneously, each identified by a unique key.

## Starting the Server

```bash
bundlebase mcp --bundle <path> [options]
```

### Flags

| Flag | Default | Description |
|---|---|---|
| `--bundle <path>` | *(none)* | Pre-open a bundle at startup (optional) |
| `--read-only` | `false` | Open in read-only mode (only with `--bundle`) |
| `--config <path>` | *(none)* | Path to a YAML/JSON config file |
| `--log-level <level>` | `ui` | Logging level |

### Examples

```bash
# Start without a bundle — agent uses open_bundle/create_bundle tools
bundlebase mcp

# Start with a bundle pre-opened (key defaults to directory name "my-bundle")
bundlebase mcp --bundle ./my-bundle

# Start read-only
bundlebase mcp --bundle ./my-bundle --read-only
```

## Available Tools

The MCP server exposes the following tools to AI assistants. All per-bundle tools require a `bundle` parameter that identifies which open bundle to operate on.

| Tool | Parameters | Description |
|---|---|---|
| `create_bundle` | `bundle` (string), `path` (string) | Create a new bundle with the given identifier |
| `open_bundle` | `bundle` (string), `path` (string), `read_only` (bool, optional) | Open an existing bundle with the given identifier |
| `close_bundle` | `bundle` (string) | Close an open bundle by its identifier |
| `list_bundles` | *(none)* | List all open bundles with their identifier, path, name, and description |
| `query` | `bundle` (string), `sql` (string) | Execute any SQL query or bundlebase command |
| `schema` | `bundle` (string) | Get column names, data types, and nullability |
| `count` | `bundle` (string) | Get total row count |
| `sample` | `bundle` (string), `limit` (integer, optional, default 10) | Preview sample rows as JSON |
| `status` | `bundle` (string) | Show uncommitted changes |
| `history` | `bundle` (string) | Show commit history |

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

Run `bundlebase setup-agent` to automatically install the MCP server config. Or add manually to `.claude/settings.json`:

```json
{
  "mcpServers": {
    "bundlebase": {
      "command": "bundlebase",
      "args": ["mcp"]
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
      "args": ["mcp"]
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
      "args": ["mcp"]
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

!!! warning
    Do not use MCP and CLI on the same bundle simultaneously. If the MCP server has a bundle open, close it with `close_bundle` before using CLI commands on the same bundle. Both can write to the same manifest files, which can cause conflicts.

## Read-Only Mode

When started with `--read-only`, the server restricts operations:

- `SELECT` and `EXPLAIN` queries execute normally
- The `schema`, `count`, `sample`, and `history` tools work normally
- All modification commands (`ATTACH`, `FILTER`, `COMMIT`, etc.) return an error
- Useful for giving AI assistants safe, read-only access to data
