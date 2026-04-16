---
title: Bundlebase for AI Assistant Users — Query Bundles in Natural Language
description: Use an AI assistant to explore, query, and update Bundlebase bundles without writing code. Start the MCP server and your assistant gets schema, history, sample, and query as native tools.
---

# Use Case: AI Assistant User

You have a bundle. You want to ask questions about the data, explore it, and get answers — without writing SQL or Python yourself. Start the MCP server once and your AI assistant (Claude, ChatGPT, or any MCP-compatible assistant) gets direct access to the bundle.

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
| `status` | Bundle path, version, row count, last commit |
| `schema` | Column names and types |
| `sample` | First N rows as a preview |
| `history` | Full commit log |
| `query` | Run any SQL against the bundle |

Point your AI assistant at the running MCP server and start asking questions.

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
Add an index on region so regional queries are faster.
```

```
Index created on 'region'. Queries filtering by region will now use the
index instead of scanning the full dataset.
```

## Generating reports

Ask the assistant to produce a formatted PDF report from the bundle — it writes the report template and runs `bundlebase generate-report` for you:

```
Generate a PDF report on Q1 sales performance. Include a bar chart of deals by region,
a trend line of deal size over the quarter, and a table of the top 20 deals by amount.
Save it as q1-sales-report.pdf.
```

```
I'll create a report from the sales-q1 bundle. Here's what I'm generating:

  - Bar chart: deal count by region
  - Line chart: average deal size by week across Q1
  - Table: top 20 deals by amount with account name and close date

Writing report template and running bundlebase generate-report...

Done. q1-sales-report.pdf saved — 3 charts, 18,423 rows of live data,
generated at 2026-04-05 14:32.
```

You can be more specific about what you want:

```
Add a section breaking down deals by customer tier, and filter the trend chart
to North America only. Regenerate the report.
```

```
Updated report generated. Changes applied:
  - Added "Deals by Customer Tier" horizontal bar chart after the region chart
  - Trend line now filters to region = 'NA' (8,841 deals)

q1-sales-report.pdf overwritten.
```

Reports pull live data from the bundle at generation time — if the bundle is updated next month, regenerating the report picks up the new data automatically.

## Session recovery

If a previous session failed mid-run, the bundle is always in the last committed state — no partial data to clean up. Just ask:

```
What's the current state of the bundle? Did the last update complete?
```

```
Bundle is at v3 (2026-04-01): "Added March data" — 18,423 rows.
This is a clean committed state. If a previous session failed, it left no
partial data behind.
```

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

- [Python Quick Start](../getting-started/python/quick-start.md) — if you want to build bundles programmatically
- [Reports](../guide/reports.md) — chart types and report template options
- [SQL Reference](../sql-reference/index.md) — full SQL syntax the assistant uses under the hood
- [Why Bundlebase?](../getting-started/why-bundlebase.md#ai-assistant-integration-mcp) — how MCP compares to other approaches
