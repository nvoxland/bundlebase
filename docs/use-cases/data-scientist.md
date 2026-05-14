---
title: Bundlebase for Data Scientists — Organize and Share Datasets
description: How data scientists use Bundlebase to combine messy CSV and Parquet sources, normalize schemas, filter and document transformations, and share versioned datasets with teammates via S3 or a local path.
---

# Use Case: Data Scientist

## The problem

You have data from multiple places — CSVs from a vendor, Parquet exports from a database, maybe a direct API pull. You've cleaned it up and done your analysis. Now someone asks: "can you share that dataset?" And you realize: the "dataset" is actually a Python script, three files, and a notebook with a dozen transformation steps embedded in it.

Bundlebase gives you a place to land the cleaned, combined data so that anyone with the path can pick up exactly what you worked with — including what you filtered out, what you renamed, and when things changed.

## The scenario

You're analyzing customer support tickets. The data lives in three places:

- Monthly CSV exports from your ticketing system (inconsistent column names between months)
- A product lookup table in Parquet on S3
- A filter: only tickets from enterprise customers, resolved status

You want to share a stable, versioned view of this with the engineering team who's building a dashboard.

## Step 1: Combine and clean the raw data

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = (bb.create("s3://team-data/support-tickets")
        .attach("exports/2026-01.csv")
        .attach("exports/2026-02.csv")
        .attach("exports/2026-03.csv")
        .normalize_column_names()          # fixes "Ticket ID", "ticket_id", "TicketId" -> consistent
        .filter("customer_tier = 'enterprise' AND status = 'resolved'")
        .drop_column("internal_notes")     # strip content you don't want shared
        .drop_column("assignee_email"))
    ```

=== "SQL"

    ```sql
    CREATE 's3://team-data/support-tickets';
    ATTACH 'exports/2026-01.csv';
    ATTACH 'exports/2026-02.csv';
    ATTACH 'exports/2026-03.csv';
    NORMALIZE COLUMN NAMES;
    FILTER WITH SELECT * FROM bundle WHERE customer_tier = 'enterprise' AND status = 'resolved';
    DROP COLUMN internal_notes;
    DROP COLUMN assignee_email;
    ```

!!! tip "Why normalize first"
    CSVs from the same system often have inconsistent column casing between exports. `normalize_column_names()` handles this in one step so your filter and drop operations use predictable names.

## Step 2: Enrich with a lookup table

The tickets have a `product_id` column but the dashboard needs the product name. You have a product lookup table in S3:

=== "Python"

    ```python
    bundle.join("products",
        on="bundle.product_id = products.id",
        location="s3://team-data/products/lookup.parquet",
        how="left")
    ```

=== "SQL"

    ```sql
    JOIN 's3://team-data/products/lookup.parquet' AS products ON bundle.product_id = products.id;
    ```

The left join keeps all tickets — even the ones where the product ID doesn't match anything in the lookup, which is worth knowing about.

## Step 3: Document and commit

=== "Python"

    ```python
    bundle.set_name("Support Tickets — Enterprise Resolved Q1 2026")
    bundle.set_description(
        "Monthly CSV exports from Zendesk, normalized columns, "
        "filtered to enterprise/resolved, joined with product lookup. "
        "internal_notes and assignee_email removed."
    )
    bundle.commit("Initial Q1 2026 export")
    ```

=== "SQL"

    ```sql
    SET NAME 'Support Tickets — Enterprise Resolved Q1 2026';
    SET DESCRIPTION 'Monthly CSV exports from Zendesk, normalized columns, filtered to enterprise/resolved, joined with product lookup. internal_notes and assignee_email removed.';
    COMMIT 'Initial Q1 2026 export';
    ```

The description travels with the data. Six months from now, someone opening this bundle doesn't need to find you to understand what's in it.

## Step 4: Share it

Send the path: `s3://team-data/support-tickets`

The engineering team opens it:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://team-data/support-tickets")
    print(bundle.name)
    print(bundle.description)
    print(bundle.num_rows)

    df = bundle.to_pandas()
    # or
    df = bundle.to_polars()
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/support-tickets';
    SHOW STATUS;
    SELECT * FROM bundle;
    ```

They can also query it directly without loading everything into memory:

=== "Python"

    ```python
    # What are the most common issue types?
    by_type = bundle.query("""
        SELECT issue_type, COUNT(*) as count, AVG(resolution_hours) as avg_hours
        FROM bundle
        GROUP BY issue_type
        ORDER BY count DESC
    """).to_pandas()
    ```

=== "SQL"

    ```sql
    SELECT issue_type, COUNT(*) as count, AVG(resolution_hours) as avg_hours
    FROM bundle
    GROUP BY issue_type
    ORDER BY count DESC;
    ```

## Step 5: Update it when data changes

When April's export arrives, you don't create a new bundle — you extend the existing one:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://team-data/support-tickets").extend()
    bundle.attach("exports/2026-04.csv")
    bundle.commit("Added April 2026 data")
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/support-tickets';
    EXTEND;
    ATTACH 'exports/2026-04.csv';
    COMMIT 'Added April 2026 data';
    ```

Anyone who already has the path gets the update automatically next time they open it. The history is preserved:

=== "Python"

    ```python
    bundle = bb.open("s3://team-data/support-tickets")
    for entry in bundle.history():
        print(entry)
    # v1: Initial Q1 2026 export
    # v2: Added April 2026 data
    ```

=== "SQL"

    ```sql
    SHOW HISTORY;
    ```

## What you've avoided

- No "which file is the latest?" confusion — the bundle is always the latest committed state
- No "what did you filter?" questions — the commit history and description capture it
- No re-running your cleaning script to hand off data — commit once, share the path
- No schema drift between months — `normalize_column_names()` is idempotent and committed

## Next steps

- [Attaching data](../guide/attaching.md) — full options for attach, including column type casting
- [Joins](../guide/joins.md) — joining with other bundles or files
- [Versioning](../guide/versioning.md) — reset, undo, and viewing history
- [Querying](../guide/querying.md) — SQL syntax and output formats
