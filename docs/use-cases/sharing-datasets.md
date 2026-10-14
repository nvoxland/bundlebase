---
title: Bundlebase for Sharing Datasets — A Path Instead of a Database
description: Clean and combine data once, commit it, and share a path. The people who open it get the data, what the columns mean, how it was prepared, and a version they can cite. No database, no export, no README to keep in sync.
seo_title: "Bundlebase for Sharing Datasets — A Path Instead of a Database"
---

# Use Case: Sharing a Dataset

## The problem

Someone asks you to share the dataset behind your analysis. You realize the "dataset" is a Python script, three source files, and a notebook with a dozen cleaning steps in it. Sending the output as a CSV loses everything that explains it, and it is stale the next time the source updates.

From the other side it looks just as bad. To analyze data someone else prepared, you get added to a database, which takes a week, or you get an export, which is stale on arrival, or you dig through a bucket and guess what the files mean.

A bundle is one path that carries the data, what it means, how it was prepared, and every version of it. The person preparing it commits once. The person using it calls `open`.

## Preparing the dataset

You are analyzing support tickets. Monthly CSV exports have inconsistent column names, a product lookup lives in Parquet on S3, and the engineering team only needs resolved tickets from enterprise customers.

### Combine and clean

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = (bb.create("s3://team-data/support-tickets")
        .attach("exports/2026-01.csv")
        .attach("exports/2026-02.csv")
        .attach("exports/2026-03.csv")
        .normalize_column_names()          # "Ticket ID", "TicketId" -> ticket_id
        .filter("SELECT * FROM bundle WHERE customer_tier = 'enterprise' AND status = 'resolved'")
        .drop_column("internal_notes")
        .drop_column("assignee_email"))

    bundle.join("products",
        "bundle.product_id = products.id",
        location="s3://team-data/products/lookup.parquet",
        join_type="left")
    ```

=== "SQL"

    ```bash
    bundlebase repl --bundle s3://team-data/support-tickets --create
    ```

    ```sql
    ATTACH 'exports/2026-01.csv';
    ATTACH 'exports/2026-02.csv';
    ATTACH 'exports/2026-03.csv';
    NORMALIZE COLUMN NAMES;
    FILTER WITH SELECT * FROM bundle WHERE customer_tier = 'enterprise' AND status = 'resolved';
    DROP COLUMN internal_notes;
    DROP COLUMN assignee_email;
    JOIN 's3://team-data/products/lookup.parquet' AS products ON bundle.product_id = products.id;
    ```

### Say what it means

This is the step that usually lives in a README, if it exists at all. In a bundle it is part of the data:

=== "Python"

    ```python
    bundle.set_name("Support Tickets — Enterprise Resolved Q1 2026")
    bundle.set_description("Zendesk monthly exports, enterprise and resolved only, joined with product lookup.")
    bundle.describe_column("resolution_hours", "wall-clock hours from creation to resolved, including weekends")
    bundle.define_metric("median_resolution", "APPROX_MEDIAN(resolution_hours)",
                         "median hours to resolve; use instead of AVG, which outliers skew")
    bundle.commit("Initial Q1 2026 export")
    ```

=== "SQL"

    ```sql
    SET NAME 'Support Tickets — Enterprise Resolved Q1 2026';
    SET DESCRIPTION 'Zendesk monthly exports, enterprise and resolved only, joined with product lookup.';
    DESCRIBE COLUMN resolution_hours AS 'wall-clock hours from creation to resolved, including weekends';
    CREATE METRIC median_resolution AS 'APPROX_MEDIAN(resolution_hours)'
        DESCRIPTION 'median hours to resolve; use instead of AVG, which outliers skew';
    COMMIT 'Initial Q1 2026 export';
    ```

Six months from now, someone opening this bundle does not need to find you to learn what `resolution_hours` counts or which average to use.

### Share the path

Send `s3://team-data/support-tickets`. That is the whole handoff.

### Update it

When April arrives, add to the same bundle:

=== "Python"

    ```python
    bundle = bb.open("s3://team-data/support-tickets").extend()
    bundle.attach("exports/2026-04.csv")
    bundle.commit("Added April 2026 data")
    ```

=== "SQL"

    ```sql
    ATTACH 'exports/2026-04.csv';
    COMMIT 'Added April 2026 data';
    ```

The filter, the dropped columns, and the join apply to the new file automatically. Everyone with the path sees the update the next time they open it, and the history shows exactly what changed.

## Using the dataset

### Open it and see what you have

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://team-data/support-tickets")
    print(bundle.name)
    print(bundle.description)
    print(f"{bundle.num_rows():,} rows")
    print(bundle.version)
    ```

=== "SQL"

    ```bash
    bundlebase repl --bundle s3://team-data/support-tickets
    ```

    ```sql
    SHOW DETAILS;
    SHOW COLUMNS;
    SHOW HISTORY;
    ```

### Query it

Queries stream, so you do not need to load the whole dataset to answer a question:

=== "Python"

    ```python
    by_type = bundle.query("""
        SELECT issue_type, COUNT(*) AS tickets, APPROX_MEDIAN(resolution_hours) AS median_hours
        FROM bundle
        GROUP BY issue_type
        ORDER BY median_hours DESC
    """).to_pandas()

    df = bundle.to_polars()   # or to_pandas(), or stream_batches() for larger-than-RAM data
    ```

=== "SQL"

    ```sql
    SELECT issue_type, COUNT(*) AS tickets, APPROX_MEDIAN(resolution_hours) AS median_hours
    FROM bundle
    GROUP BY issue_type
    ORDER BY median_hours DESC;
    ```

### Cite the version you used

Record `bundle.version` alongside your results. If someone later asks which data an analysis used, you have an exact answer, and anyone can check whether the bundle has changed since.

```python
print(f"Analysis based on bundle version {bundle.version}")
```

### Add your own data without touching theirs

`extend()` gives you a copy you can change. Commit it to your own path and the shared bundle is untouched:

```python
mine = bb.open("s3://team-data/support-tickets").extend("s3://my-team/tickets-annotated")
mine.join("annotations", "bundle.ticket_id = annotations.ticket_id",
          location="my_team_annotations.csv")
mine.commit("Enriched with team annotations")
```

## What you avoided

- **"Which file is the latest?"** The path always opens the latest commit.
- **"What did you filter?"** The history and description say.
- **"What does this column mean?"** The column says.
- **Re-running the cleaning script for every handoff.** Commit once, share the path.

## Next steps

- [Attaching data](../guide/attaching.md) — attach options, including type casting
- [Joins](../guide/joins.md) — joining other files and bundles
- [Semantic Model](../guide/semantic-model.md) — column descriptions and named metrics
- [Versioning](../guide/versioning.md) — history, undo, and reset
- [Jupyter](../guide/jupyter.md) — querying bundles in notebook cells
