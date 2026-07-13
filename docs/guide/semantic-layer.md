---
seo_title: "Semantic Layer: Column Descriptions and Named Metrics for AI Agents - Bundlebase"
---

# Semantic Layer

A bundle can carry what its data *means*, not just its shape. Two things make up the semantic layer:

- **Column descriptions** — what a column actually is (`amt` is "order total in USD, net of refunds").
- **Named metrics** — the canonical way to compute a number (`total_revenue` is `SUM(amount)`, and nothing else).

Both are recorded as operations, so they travel inside the bundle, are versioned like any other change, and survive commit and reopen. Anyone who opens the bundle — a teammate, a BI tool, or an AI agent — gets the same definitions.

## Why it matters

A bare schema tells a reader that a column is called `amt` and holds a number. It does not say whether that number includes refunds, tax, or shipping — so two people asking "what was revenue?" write two different `SUM`s and get two different answers, both of which look right.

This is especially acute for AI assistants. Given only column names and types, an assistant writes plausible SQL that can quietly disagree with the ground truth. Giving it the column's meaning and the author's canonical metric definition is what turns a plausible answer into a correct one.

## Describing columns

Attach a description to any column. A later `DESCRIBE COLUMN` on the same column replaces the earlier one.

=== "SQL"

    ```sql
    DESCRIBE COLUMN amount AS 'order total in USD, net of refunds'
    DESCRIBE COLUMN status AS 'one of: pending, shipped, cancelled'
    ```

=== "Async API"

    ```python
    c = await c.describe_column("amount", "order total in USD, net of refunds")
    ```

=== "Sync API"

    ```python
    c.describe_column("amount", "order total in USD, net of refunds")
    ```

## Defining metrics

A metric is a named, reusable aggregate expression — a *measure*. Unlike a view (a whole query), a metric is a single expression meant to be composed with different dimensions: group it by region this time, by month the next. A later `CREATE METRIC` with the same name replaces the earlier one.

=== "SQL"

    ```sql
    CREATE METRIC total_revenue AS 'SUM(amount)' DESCRIPTION 'gross revenue in USD'
    CREATE METRIC avg_order_value AS 'SUM(amount) / COUNT(DISTINCT order_id)'

    DROP METRIC avg_order_value
    ```

=== "Async API"

    ```python
    c = await c.define_metric("total_revenue", "SUM(amount)", "gross revenue in USD")
    ```

=== "Sync API"

    ```python
    c.define_metric("total_revenue", "SUM(amount)", "gross revenue in USD")
    ```

## How consumers see it

**AI assistants (MCP).** The `describe` tool returns the schema *with* column descriptions, plus the list of metrics — so the assistant knows what the data is before it writes any SQL. The `metric` tool then computes a metric by name, using the bundle author's definition rather than one the assistant invented:

```
metric(bundle="sales", name="total_revenue")
metric(bundle="sales", name="total_revenue", dimensions=["region"])
```

See the [MCP Server Guide](cli-mcp.md).

**BI tools (Arrow Flight SQL).** Column descriptions are exported as Arrow field metadata under the key `description`, so clients that read field metadata display them alongside the column. See [Column Descriptions in BI Tools](cli-serve.md#column-descriptions-in-bi-tools).

## Practical advice

Describe the columns whose meaning is not obvious from the name, and define the metrics people will actually ask for. A bundle whose numbers everyone computes the same way is worth considerably more than one that merely holds the rows.
