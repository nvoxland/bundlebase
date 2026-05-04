---
title: "Example: Claude Code History"
description: "End-to-end example: a Go connector that flattens Claude Code JSONL transcripts, registered via IMPORT CONNECTOR with a bundled source archive, ready for FETCH on the recipient's machine."
---

# Claude Code Transcript History

This example builds an **empty** Bundlebase bundle that anyone with a Claude Code installation can run `FETCH` against to load their own transcript history into a queryable schema.

The bundle ships:

- A native Go connector (FFI shared library) that walks Claude's `~/.claude/projects` JSONL files and flattens them into one row per transcript event.
- A `CREATE SOURCE` definition that points the connector at the local `~/.claude/projects` directory.
- B-tree indexes on `project_id`, `session_id`, `event_type`, `timestamp` and an inverted text index on `search_text`.
- Two views (`message_events`, `tool_events`) for the most common slice queries.
- The Go source for the connector, attached as a `WITH (src = '…')` archive so a recipient can audit, fork, or rebuild the binary.

No data is fetched at build time, so the bundle ends as a structure-only / empty bundle. Recipients run `FETCH base ADD` to populate it from their own machine.

## Files

| File | Description                                                                         | Download |
|---|-------------------------------------------------------------------------------------|---|
| <span style="white-space: nowrap">`claude-history-bundle.tar.gz`</span> | **Prebuilt** empty bundle (gzipped tar). Extract, open with `bundlebase`, and `FETCH`. | [Download](scripts/claude_history/claude-history-bundle.tar.gz){:download="claude-history-bundle.tar.gz"} |
| <span style="white-space: nowrap">`bundlebase.yaml`</span> | Config file enabling `system.allow_external_code` so the bundled FFI connector loads. | [Download](scripts/claude_history/bundlebase.yaml){:download="bundlebase.yaml"} |
| <span style="white-space: nowrap">`create_claude_history_bundle.py`</span> | Python script that builds the connector, defines the bundle, and writes the `.tar`. | [Download](scripts/claude_history/create_claude_history_bundle.py){:download="create_claude_history_bundle.py"} |
| <span style="white-space: nowrap">`claude_history_connector/main.go`</span> | Go connector source. Implements the bundlebase FFI ABI to walk `*.jsonl` files.     | [Download](scripts/claude_history/claude_history_connector/main.go){:download="main.go"} |
| <span style="white-space: nowrap">`claude_history_connector/go.mod`</span> | Go module manifest.                                                                 | [Download](scripts/claude_history/claude_history_connector/go.mod){:download="go.mod"} |
| <span style="white-space: nowrap">`claude_history_connector/go.sum`</span> | Go module checksums.                                                                | [Download](scripts/claude_history/claude_history_connector/go.sum){:download="go.sum"} |

## Using the Prebuilt Bundle

Download `claude-history-bundle.tar.gz` and extract it before use — FFI shared libraries can't be `dlopen`-ed from inside a tar:

```bash
mkdir claude-history-bundle && tar -xzf claude-history-bundle.tar.gz -C claude-history-bundle
```

Then `FETCH` to populate the bundle from your local `~/.claude/projects` directory and start querying. Pick whichever interface you prefer:

The connector is a custom FFI shared library, so bundlebase has to be told it's allowed to load external code by setting `system.allow_external_code = 'true'`. The CLI tab below picks that up from a YAML config file ([`bundlebase.yaml`](scripts/claude_history/bundlebase.yaml){:download="bundlebase.yaml"}) via `--config`; SQL uses `SET CONFIG`; Python passes a `config={...}` dict to `open()`. Without it, `FETCH` refuses to load the connector.

=== "CLI"

    Download [`bundlebase.yaml`](scripts/claude_history/bundlebase.yaml){:download="bundlebase.yaml"} into the same directory as the extracted bundle and pass it with `--config`:

    ```bash
    # Pull data from your local ~/.claude/projects into the bundle.
    # `extend` is the mutating subcommand; `--config` enables the FFI connector.
    bundlebase extend --config bundlebase.yaml \
        --bundle ./claude-history-bundle \
        "FETCH base ADD"

    # Same --config is needed on every read that touches the connector.
    bundlebase query --config bundlebase.yaml \
        --bundle ./claude-history-bundle \
        "SELECT COUNT(*) FROM bundle"

    # Recent assistant replies in one project (uses the project_id index)
    bundlebase query --config bundlebase.yaml \
        --bundle ./claude-history-bundle "
            SELECT timestamp, content_text
            FROM message_events
            WHERE project_id = '/Users/me/src/myproj'
              AND message_role = 'assistant'
            ORDER BY timestamp DESC
            LIMIT 10
        "
    ```

    The config file is just two lines of YAML — `bundlebase.yaml` contains:

    ```yaml
    system:
      allow_external_code: 'true'
    ```

=== "SQL"

    Run these from the REPL (`bundlebase repl --bundle ./claude-history-bundle`) or any tool that submits SQL to the bundle. The `SET CONFIG` is session-scoped, so it only needs to run once per connection.

    ```sql
    -- Allow loading the bundled FFI connector. Runtime-only (session scope).
    SET CONFIG allow_external_code = 'true' FOR 'system';

    -- Pull data from your local ~/.claude/projects into the bundle
    FETCH base ADD;

    -- Quick sanity-check
    SELECT COUNT(*) FROM bundle;

    -- Recent assistant replies in one project (uses the project_id index)
    SELECT timestamp, content_text
    FROM message_events
    WHERE project_id = '/Users/me/src/myproj'
      AND message_role = 'assistant'
    ORDER BY timestamp DESC
    LIMIT 10;
    ```

=== "Python"

    ```python
    import bundlebase.sync as bb

    # `config={"system": {"allow_external_code": "true"}}` is required so
    # bundlebase will load the bundled FFI connector. extend() turns the
    # read-only Bundle into a BundleBuilder so we can fetch and commit.
    bundle = bb.open(
        "./claude-history-bundle",
        config={"system": {"allow_external_code": "true"}},
    ).extend()

    # Pull data from your local ~/.claude/projects into the bundle
    bundle.fetch("base", "add")
    bundle.commit("Loaded transcripts")

    # Quick sanity-check
    print(bundle.query("SELECT COUNT(*) FROM bundle"))

    # Recent assistant replies in one project (uses the project_id index)
    print(bundle.query(
        """
        SELECT timestamp, content_text
        FROM message_events
        WHERE project_id = '/Users/me/src/myproj'
          AND message_role = 'assistant'
        ORDER BY timestamp DESC
        LIMIT 10
        """
    ))
    ```

The bundle already carries the source definition, the connector binary, and the indexes and views — so as soon as `FETCH` finishes, your data is queryable through `message_events`, `tool_events`, and the indexed columns.

The connector binary is platform-specific — the prebuilt tar ships only for the platform it was built on. To target other platforms, run the script yourself.


## Querying the Bundle

Queries always run against the table named `bundle` — that's the full row set. The b-tree indexes on `project_id`, `session_id`, `event_type`, and `timestamp` make filters on those columns cheap, and the inverted text index on `search_text` powers full-text search. Every session that does a `fetch` needs `SET CONFIG allow_external_code = 'true' FOR 'system'` first (or the equivalent Python `config=...` arg).

```sql
-- All assistant replies in one project, newest first.
-- `event_type` mirrors the JSONL `type` field — assistant/user messages
-- have event_type = 'assistant' or 'user' (there is no 'message').
SELECT timestamp, content_text
FROM bundle
WHERE project_id = '/Users/me/src/myproj'
  AND event_type = 'assistant'
ORDER BY timestamp DESC
LIMIT 50;

-- Distribution of event types across the whole bundle
SELECT event_type, COUNT(*) AS n
FROM bundle
GROUP BY event_type
ORDER BY n DESC;

-- Sessions ranked by how many tool calls they made
-- (tool calls live on assistant rows; tool_names is non-null only for them).
SELECT session_id, COUNT(*) AS tool_calls
FROM bundle
WHERE event_type = 'assistant' AND tool_names IS NOT NULL
GROUP BY session_id
ORDER BY tool_calls DESC
LIMIT 20;

-- Token spend per day across every project
SELECT date_trunc('day', timestamp) AS day,
       SUM(message_usage_input_tokens)  AS input_tokens,
       SUM(message_usage_output_tokens) AS output_tokens
FROM bundle
WHERE event_type = 'assistant'
GROUP BY 1
ORDER BY 1 DESC;

-- Full-text search uses the `search()` table function, which replaces
-- `FROM bundle` and exposes a BM25 `_score` column. Single-arg form is
-- enough here because the bundle has exactly one inverted index.
SELECT _score, timestamp, project_id, event_type, tool_names, content_text
FROM search('web_search')
ORDER BY _score DESC
LIMIT 100;

-- Full-text search combined with an indexed filter (search() is just a
-- table source — normal WHERE/ORDER BY clauses compose with it).
SELECT _score, timestamp, content_text
FROM search('pg_dump')
WHERE project_id = '/Users/me/src/myproj'
ORDER BY _score DESC
LIMIT 50;

-- Two-arg form names the index explicitly and accepts BM25 query syntax
-- (field:term, AND/OR, etc.) — useful when a bundle has multiple text indexes.
SELECT _score, timestamp, content_text
FROM search('search_text_idx', 'web_search AND timeout')
ORDER BY _score DESC
LIMIT 20;

-- Errors surfaced from tool results
SELECT timestamp, session_id, tool_names, tool_result_error
FROM bundle
WHERE tool_result_error IS NOT NULL
ORDER BY timestamp DESC
LIMIT 50;
```

### Using the bundled views

`message_events` and `tool_events` are predefined views, but you don't `SELECT FROM message_events` — you scope the bundle to the view first, then query `bundle` against that scope. From Python:

```python
import bundlebase as bb

bundle = bb.open("./claude-history-bundle", config={"system": {"allow_external_code": "true"}})

messages = bundle.view("message_events")
print(messages.query("""
    SELECT timestamp, message_role, content_text
    FROM bundle
    WHERE project_id = '/Users/me/src/myproj'
    ORDER BY timestamp DESC
    LIMIT 50
"""))

tools = bundle.view("tool_events")
print(tools.query("SELECT tool_names, COUNT(*) FROM bundle GROUP BY tool_names ORDER BY 2 DESC"))
```

`bundle.view(name)` returns a read-only sub-bundle that exposes the view's filtered/projected rows under the same `bundle` table name, so the same SQL idioms apply.

## Building the Bundle Yourself

From this example's `scripts/claude_history/` directory:

```bash
# Build into the default ./claude-history-bundle directory and tar
python create_claude_history_bundle.py
```

The script needs the [Go toolchain](https://go.dev/doc/install) on your `PATH` to compile the connector and `bundlebase` (with `allow_external_code = true`) to register it.

The script writes a directory bundle and then calls `bundle.export_tar(...)` to produce a single-file copy for distribution. The directory bundle is what you actually use locally — the tar exists so you can hand a single artifact to someone else.

## The Build Script

The Python script runs in three phases:

1. **Build a full bundle locally.** `CREATE SOURCE` auto-fetches your own transcripts so the bundle has a real schema. `CREATE INDEX` and `CREATE VIEW` are then applied against that schema (they need to resolve column references, which an empty bundle can't do).
2. **`EXPORT EMPTY`** to a sibling path. The export walks the full bundle's history and re-applies only the *structural* operations (CREATE SOURCE, CREATE INDEX, CREATE VIEW, IMPORT CONNECTOR, EXPECTED SCHEMA, etc.) into a fresh bundle. ATTACH/DETACH/REPLACE/DELETE/UPDATE are stripped, so the result has no rows but knows how to fetch them.
3. **`export_tar`** packages the empty bundle as a single `.tar` for download.

```python title="create_claude_history_bundle.py"
--8<-- "docs/examples/scripts/claude_history/create_claude_history_bundle.py"
```

## The Go Connector

The connector implements the [native FFI ABI](../guide/custom-connectors/native.md#c-abi-reference): `bundlebase_discover` to enumerate JSONL files in the source directory and `bundlebase_data` to stream one Arrow record batch per file. Transcript events are flattened — one row per JSON line, with nested message / tool / usage fields hoisted into top-level columns matching the indexes the bundle defines.

```go title="claude_history_connector/main.go"
--8<-- "docs/examples/scripts/claude_history/claude_history_connector/main.go"
```
