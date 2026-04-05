---
title: Bundlebase for Analysts — Self-Serve Analytics Without a Database
description: Query shared datasets with SQL, export to pandas or polars, and generate PDF reports with charts — all from a single bb.open() call. No database credentials, no CSV exports, no setup.
---

# Use Case: Analyst

## The problem

You want to do real analysis on data someone else prepared. Your options are usually: get added to a database (takes a week, requires a VPN), ask the data engineer to run an export (interrupts them, you get a stale CSV), or dig through S3 yourself (requires knowing where things are and what the files mean).

If the data has been published as a Bundlebase bundle, none of that applies. You get a path, you call `bb.open()`, and you have a queryable, self-describing dataset in seconds.

## Opening a shared bundle

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://team-data/support-tickets")

    # Understand what you have before loading anything
    print(bundle.name)
    print(bundle.description)
    print(f"{bundle.num_rows:,} rows")
    print(bundle.version)
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/support-tickets';
    SHOW STATUS;
    ```

```
Support Tickets — Enterprise Resolved Q1 2026
Monthly CSV exports from Zendesk, normalized columns, filtered to enterprise/resolved,
joined with product lookup. internal_notes and assignee_email removed.
47,382 rows
00003f8a1c2b
```

This is the information that usually lives in a README — or doesn't exist at all. Here it's attached to the data itself.

## Exploring the schema

=== "Python"

    ```python
    for field in bundle.schema:
        print(f"  {field.name}: {field.data_type}")
    ```

=== "SQL"

    ```sql
    SHOW SCHEMA;
    ```

```
  ticket_id: Int64
  created_at: Timestamp
  product_id: Utf8
  product_name: Utf8
  issue_type: Utf8
  priority: Utf8
  resolution_hours: Float64
  customer_tier: Utf8
  status: Utf8
```

## Querying with SQL

Bundlebase supports full SQL via Apache DataFusion. You don't need to load the whole dataset to answer a question:

=== "Python"

    ```python
    # What issue types take the longest to resolve?
    by_type = bundle.query("""
        SELECT issue_type,
               COUNT(*) as ticket_count,
               ROUND(AVG(resolution_hours), 1) as avg_hours,
               ROUND(PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY resolution_hours), 1) as p95_hours
        FROM bundle
        GROUP BY issue_type
        ORDER BY avg_hours DESC
    """).to_pandas()

    # Is resolution time trending up or down?
    monthly = bundle.query("""
        SELECT DATE_TRUNC('month', created_at) as month,
               COUNT(*) as tickets,
               ROUND(AVG(resolution_hours), 1) as avg_resolution_hours
        FROM bundle
        GROUP BY DATE_TRUNC('month', created_at)
        ORDER BY month
    """).to_pandas()
    ```

=== "SQL"

    ```sql
    -- What issue types take the longest to resolve?
    SELECT issue_type,
           COUNT(*) as ticket_count,
           ROUND(AVG(resolution_hours), 1) as avg_hours,
           ROUND(PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY resolution_hours), 1) as p95_hours
    FROM bundle
    GROUP BY issue_type
    ORDER BY avg_hours DESC;

    -- Is resolution time trending up or down?
    SELECT DATE_TRUNC('month', created_at) as month,
           COUNT(*) as tickets,
           ROUND(AVG(resolution_hours), 1) as avg_resolution_hours
    FROM bundle
    GROUP BY DATE_TRUNC('month', created_at)
    ORDER BY month;
    ```

Query results export to pandas, polars, or dict — whichever fits your workflow.

## Loading into pandas for exploratory analysis

For interactive exploration in a notebook:

=== "Python"

    ```python
    df = bundle.to_pandas()

    # Now use pandas as normal
    df['created_at'] = pd.to_datetime(df['created_at'])
    df['week'] = df['created_at'].dt.isocalendar().week

    weekly = df.groupby('week').agg(
        tickets=('ticket_id', 'count'),
        avg_hours=('resolution_hours', 'mean')
    ).round(1)
    ```

=== "SQL"

    ```sql
    SELECT * FROM bundle;
    ```

!!! tip "Jupyter integration"
    Bundlebase has a Jupyter extension that lets you query bundles directly in notebook cells. See the [Jupyter use case](../guide/jupyter.md).

## Checking when the data was last updated

=== "Python"

    ```python
    for entry in bundle.history():
        print(entry)
    # v1: Initial Q1 2026 export  (2026-04-01)
    # v2: Added April 2026 data   (2026-05-02)
    ```

=== "SQL"

    ```sql
    SHOW HISTORY;
    ```

If the version has changed since you last ran your analysis, you know the underlying data has been updated. You can decide whether to re-run or note the version in your report.

## Generating a PDF report

If you need to share findings as a formatted report rather than a notebook or a spreadsheet, Bundlebase can generate PDFs directly from a markdown template:

```markdown
# Support Ticket Analysis — Q1 2026

Analysis of enterprise support tickets. Data from `s3://team-data/support-tickets`.

## Volume by Issue Type

```bundlebase
bundle: s3://team-data/support-tickets
query: SELECT issue_type, COUNT(*) as tickets FROM bundle GROUP BY issue_type ORDER BY tickets DESC
type: horizontal_bar
title: Tickets by Issue Type
```

## Resolution Time Trend

```bundlebase
bundle: s3://team-data/support-tickets
query: |
  SELECT DATE_TRUNC('month', created_at) as month,
         AVG(resolution_hours) as avg_hours
  FROM bundle
  GROUP BY month ORDER BY month
type: line
title: Average Resolution Hours by Month
options:
  fill: true
  y_label: "Hours"
```

## Top 20 Longest-Running Tickets

```bundlebase
bundle: s3://team-data/support-tickets
query: SELECT ticket_id, issue_type, resolution_hours FROM bundle ORDER BY resolution_hours DESC LIMIT 20
type: table
title: Longest to Resolve
```
```

Generate the PDF:

```bash
bundlebase generate-report --input analysis.md --output support-q1-2026.pdf
```

The report pulls live data from the bundle at generation time — no manual copy-paste, no stale charts.

## Doing your own extended analysis

If you want to add your own data on top of what's been shared, you can extend the bundle without modifying the original:

=== "Python"

    ```python
    # Your own extended view — doesn't touch the shared bundle
    my_view = bb.open("s3://team-data/support-tickets").extend()
    my_view.attach("my_team_annotations.csv")
    my_view.join("annotations",
        on="bundle.ticket_id = annotations.ticket_id",
        location="my_team_annotations.csv")

    # Work with the enriched data locally
    df = my_view.to_pandas()

    # If you want to save it, commit to your own path
    my_view.commit("Enriched with team annotations")
    # (only if you called bb.create or provided a path — otherwise it stays in memory)
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/support-tickets';
    EXTEND;
    ATTACH 'my_team_annotations.csv';
    JOIN 'my_team_annotations.csv' AS annotations ON bundle.ticket_id = annotations.ticket_id;
    SELECT * FROM bundle;
    COMMIT 'Enriched with team annotations';
    ```

!!! note
    `extend()` on a committed bundle creates a local mutable copy. It doesn't modify the original. The original is read-only until whoever owns it calls `extend()` and `commit()`.

## What the version tells you

When you use a bundle in an analysis, record `bundle.version` alongside your results. This is the equivalent of pinning a dataset version — if someone asks "which data did this analysis use?", you have an exact answer:

=== "Python"

    ```python
    print(f"Analysis based on bundle version: {bundle.version}")
    # Analysis based on bundle version: 00002a9b3c1d
    ```

=== "SQL"

    ```sql
    SHOW STATUS;
    ```

Anyone can reproduce your analysis by opening the same bundle and checking out that version — or simply verifying that the version hasn't changed since your run.

## Next steps

- [Querying](../guide/querying.md) — full SQL reference and streaming output for large datasets
- [Reports](../guide/reports.md) — all chart types and report options
- [Jupyter](../guide/jupyter.md) — interactive analysis in notebooks
- [Versioning](../guide/versioning.md) — understanding commits and history
