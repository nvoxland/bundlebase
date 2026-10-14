---
title: Bundlebase for AI Agents — Answers You Can Trace Back to the Data
description: Give an AI assistant a bundle over MCP and it reads what the columns mean, how the metrics are defined, and where every row came from before it writes SQL. Every answer can be traced to a source file and a commit.
seo_title: "Bundlebase for AI Agents — Query Your Data over MCP, Correctly"
---

# Use Case: Agents That Answer Correctly

You have a bundle. You want to ask an AI assistant questions about it and trust the numbers that come back.

An assistant given a bare table writes SQL that parses and runs. Whether the result means what you asked is a separate question, and a schema can't answer it. A bundle carries the missing context: what each column means, how each metric is defined, which files every row came from, and every change made since. Over MCP the assistant reads all of it before it writes a query, and it can show you where any number came from.

If the agent is the one building the data up over many sessions, see [Agent Workspaces](agent-workspace.md).

## Setup

Run this once in your project:

```bash
bundlebase setup-agent
```

This auto-detects Claude Code and Copilot from your `PATH` and installs the local config it can manage automatically. For Copilot, that now includes both the MCP workspace config and a Bundlebase section in `AGENTS.md`. Use `--agent claude` or `--agent copilot` to target one explicitly.

For Claude Code, use `--scope global` to configure it once for all your projects instead:

```bash
bundlebase setup-agent --agent claude --scope global
```

After setup, your assistant has these tools available whenever it works with a bundle:

| Tool | What it does |
|------|-------------|
| `open_bundle` / `create_bundle` / `close_bundle` | Open, create, or close a bundle by path |
| `describe` | Full self-description: name, description, schema (with column descriptions), named metrics, data provenance, transformation record, lineage, and history -- the best first call so queries reflect what the data actually is |
| `analyze` | One-shot orientation: `describe` plus a small data sample, together |
| `data_quality` | Per-column null counts/rates and distinct counts, with all-null / constant flags — plus a `suggested_cast` for text columns that really hold numbers or dates |
| `metric` | Compute a bundle-defined named metric by name (optionally grouped by dimensions) using the author's canonical definition, so the number matches ground truth |
| `status` | Bundle status including uncommitted changes |
| `schema` | Column names and types |
| `count` | Total row count |
| `sample` | First N rows as a preview |
| `history` | Full commit log |
| `query` | Run any SQL or bundlebase command against the bundle |
| `generate_report` | Build a PDF report from a markdown template |
| `list_bundles` | List currently open bundles |
| `upgrade_bundle` | Upgrade a bundle's format version |

The assistant launches the MCP server itself from the installed config -- just start asking questions.

## Exploring what you have

You don't need to know the schema or what's in the bundle — just ask:

```
What data is in this bundle? Give me an overview.
```

```
Bundle: Q1 2026 Sales — Closed Won
Rows: 18,423
Last updated: v3 — "Added March data" (2026-04-01)

Columns: deal_id, account_name, region, product_sku, amount (Float64),
         close_date (Date), sales_rep, customer_tier

Data spans January through March 2026. All records are closed-won deals.
```

```
What changed between the first and latest version?
```

```
v1 (2026-01-15): Initial Q1 export — January data only (6,102 rows)
v2 (2026-02-12): Added February data (11,847 rows)
v3 (2026-04-01): Added March data (18,423 rows)
```

## Running analysis

```
Which region had the highest average deal size in Q1?
```

```
EMEA led with an average deal size of $84,200, followed by North America
at $71,400 and APAC at $58,900.

Top 3 regions by average deal size:
  EMEA      $84,200   (3,102 deals)
  NA        $71,400   (8,841 deals)
  APAC      $58,900   (4,210 deals)
```

```
Show me the top 10 deals by amount, with the account name and close date.
```

```
| deal_id | account_name        | amount     | close_date  |
|---------|---------------------|------------|-------------|
| D-9921  | Acme Corp           | $480,000   | 2026-03-28  |
| D-8847  | Globex Industries   | $412,500   | 2026-02-14  |
| ...     | ...                 | ...        | ...         |
```

```
Is there any seasonality in deal close dates — do more deals close at end of month?
```

```
Yes, noticeably. 34% of deals close in the last 3 days of each month,
versus an expected ~10% if evenly distributed. This pattern holds across
all three months and all regions.
```

## Digging deeper

```
Are there any data quality issues I should know about?
```

```
A few things worth flagging:

1. 47 records have amount = 0.00 — likely test records or data entry errors
2. customer_tier is null for 312 records (1.7% of total)
3. 3 records have close_date in December 2025, which is outside the Q1 2026 range
```

```
Filter out the zero-amount records and the December deals. How does that change the total pipeline value?
```

```
After filtering:
  Records removed: 50
  Remaining rows: 18,373
  Total pipeline value: $1,312,847,200 (was $1,312,847,200 — zero-amount records
  didn't affect the sum, but the December deals removed $48,200)
```

## Updating the bundle

The assistant can also make changes when given the right permissions:

```
Fetch the latest data from the source and commit it as "April refresh".
```

```
Fetched 4,218 new records from the configured source.
Bundle updated: 22,641 total rows.
Committed as v4: "April refresh"
```

```
Note that customer_tier is null for trial accounts, and define revenue as the sum of amount.
```

```
Recorded:
  DESCRIBE COLUMN customer_tier AS 'null for trial accounts'
  CREATE METRIC revenue AS 'SUM(amount)'
Committed as v5: "Document customer_tier and revenue"

Any assistant or person that opens this bundle will see both.
```

## Showing where a number came from

Any answer can be traced back. Ask:

```
Where does the EMEA average come from? Which files, and has anything changed them?
```

```
The EMEA figure is AVG(amount) over 3,102 rows from three attached files:
  exports/2026-01.csv, exports/2026-02.csv, exports/2026-03.csv

Changes since they were attached:
  v3 "Drop test deals" removed 47 rows WHERE amount = 0
  amount is described as "order total in USD, net of refunds"

VERIFY DATA: all three files still match the checksums recorded at attach.
```

The assistant gets this from `describe` and `history`, which return the source of every attached file and every operation applied, with who committed it and when. Nothing here depends on the assistant's memory of a previous conversation. It is read from the bundle each time.

## Producing a report

The assistant can also write a PDF report from the bundle with `generate_report`, pulling live data at generation time. See [Reports](../guide/reports.md) for the template format.

## Session recovery

If a previous session failed mid-run, the bundle is still at its last commit, with no partial data to clean up. [Agent Workspaces](agent-workspace.md) covers building a bundle up across many sessions.

## When to use MCP vs. the Python API

**Use MCP (this use case) when:**

- You want to explore, query, or update a bundle through conversation
- You don't want to write code
- Your AI assistant supports MCP tools natively

**Use the Python API when:**

- You're building automation or pipelines that run on a schedule
- You need programmatic control over exactly what runs
- You want to integrate bundle operations into existing code

## Next steps

- [Semantic Model](../guide/semantic-model.md) — column descriptions and named metrics
- [Agent Workspaces](agent-workspace.md) — an agent that accumulates data across sessions
- [Python Quick Start](../getting-started/python/quick-start.md) — if you want to build bundles programmatically
- [SQL Reference](../sql-reference/index.md) — full SQL syntax the assistant uses under the hood
- [Why Bundlebase?](../getting-started/why-bundlebase.md#ai-assistant-integration-mcp) — how MCP compares to other approaches
