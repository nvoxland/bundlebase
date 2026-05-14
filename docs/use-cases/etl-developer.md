---
title: Bundlebase as an ETL Layer — Standardized Data Collection Without Custom Pipelines
description: Use Bundlebase to pull from SFTP, S3, HTTP APIs, and other sources on a schedule. Downstream systems read from a standardized bundle without knowing or caring about the upstream sources.
---

# Use Case: ETL / Periodic Data Collection

## The problem

You need to pull data from an external source regularly — a vendor drops CSV files on an SFTP server every night, a government API publishes a monthly export, a partner sends Parquet files to an S3 bucket. You write a script: connect to SFTP, list new files, download them, normalize the columns, load into your system. It works.

Then three months later the vendor changes their column names. Or adds a subfolder. Or the SFTP credentials rotate. Or you need to add a second vendor with a slightly different format. The custom logic accumulates.

With Bundlebase, you define the source once in a bundle. The bundle knows how to fetch; downstream systems just call `bb.open()`. The upstream plumbing is isolated in one place and the downstream interface stays the same.

## The scenario

A vendor delivers daily sales CSVs to an SFTP server. The files land in `/outbound/sales/` with names like `sales_2026-04-04.csv`. Your data science team and a reporting dashboard both need this data. You want:

- Automatic pickup of new files each day
- Consistent column names regardless of what the vendor sends
- Downstream consumers that don't know or care that SFTP is involved

## Step 1: Define the bundle and its source (once)

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = (bb.create("s3://team-data/vendor-sales")
        .set_name("Vendor Sales — Daily Feed")
        .set_description(
            "Daily CSV feed from Vendor A via SFTP. "
            "Normalized columns, filtered to completed transactions. "
            "Downstream consumers use bb.open() — SFTP details are internal."
        )
        .create_source("sftp_directory", {
            "url": "sftp://vendor.example.com/outbound/sales/",
            "patterns": "sales_*.csv",
            "username": "pipeline-user",
            "key_path": "/etc/keys/vendor_rsa"
        })
        .normalize_column_names()
        .filter("status = 'completed'")
        .drop_column("internal_vendor_code"))

    # First fetch — picks up all existing files
    bundle.fetch("base", "add")
    bundle.commit("Initial load — all historical files")
    print(f"Loaded: {bundle.num_rows:,} rows")
    ```

=== "SQL"

    ```sql
    CREATE 's3://team-data/vendor-sales';
    SET NAME 'Vendor Sales — Daily Feed';
    SET DESCRIPTION 'Daily CSV feed from Vendor A via SFTP. Normalized columns, filtered to completed transactions. Downstream consumers use OPEN — SFTP details are internal.';
    CREATE SOURCE FOR base USING sftp_directory WITH (url = 'sftp://vendor.example.com/outbound/sales/', patterns = 'sales_*.csv', username = 'pipeline-user', key_path = '/etc/keys/vendor_rsa');
    NORMALIZE COLUMN NAMES;
    FILTER WITH SELECT * FROM bundle WHERE status = 'completed';
    DROP COLUMN internal_vendor_code;
    FETCH base ADD;
    COMMIT 'Initial load — all historical files';
    SHOW STATUS;
    ```

The `normalize_column_names()` and `filter()` calls are part of the bundle definition — they run every time data is fetched. If the vendor changes `Status` to `status` or `STATUS`, it doesn't matter. The downstream schema stays consistent.

## Step 2: Daily fetch (the only recurring step)

The daily job is three lines:

=== "Python"

    ```python
    #!/usr/bin/env python3
    # scripts/fetch_vendor_sales.py

    import bundlebase.sync as bb
    from datetime import date

    bundle = bb.open("s3://team-data/vendor-sales").extend()
    bundle.fetch("base", "add")   # only pulls files not already in the bundle
    bundle.commit(f"Daily fetch — {date.today()}")

    print(f"Done: {bundle.num_rows:,} total rows at version {bundle.version}")
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/vendor-sales';
    EXTEND;
    FETCH base ADD;
    COMMIT 'Daily fetch';
    SHOW STATUS;
    ```

`fetch("base", "add")` is idempotent: it checks which files are already attached and skips them. Run it twice and nothing changes. If today's file hasn't arrived yet, nothing happens — run it again later.

## Step 3: Downstream consumers don't change

Your data science team:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://team-data/vendor-sales")
    df = bundle.to_pandas()
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/vendor-sales';
    SELECT * FROM bundle;
    ```

Your reporting dashboard (using the CLI SQL server):

```bash
bundlebase serve --bundle s3://team-data/vendor-sales --port 32010
```

A BI tool connecting over SQL gets the same data the same way. Neither consumer knows the data comes from SFTP. If you switch vendors, update the source definition in the bundle — the downstream interface is unchanged.

## Handling multiple upstream sources

When you add a second vendor with a different format, you extend the same bundle rather than creating a parallel pipeline:

=== "Python"

    ```python
    bundle = bb.open("s3://team-data/vendor-sales").extend()

    # Add a second vendor source — different SFTP, different format
    bundle.create_source("sftp_directory", {
        "url": "sftp://vendor-b.example.com/exports/",
        "patterns": "*.parquet",
        "username": "pipeline-b",
        "key_path": "/etc/keys/vendor_b_rsa"
    }, name="vendor_b")

    bundle.fetch("vendor_b", "add")
    bundle.commit("Added Vendor B source")
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/vendor-sales';
    EXTEND;
    CREATE SOURCE FOR vendor_b USING sftp_directory WITH (url = 'sftp://vendor-b.example.com/exports/', patterns = '*.parquet', username = 'pipeline-b', key_path = '/etc/keys/vendor_b_rsa');
    FETCH vendor_b ADD;
    COMMIT 'Added Vendor B source';
    ```

Both sources feed the same bundle. Downstream consumers still call `bb.open("s3://team-data/vendor-sales")` and get unified data — they don't need to know a second vendor was added.

## The abstraction it provides

```
                    ┌─────────────────────────────────────┐
Upstream sources    │  SFTP (Vendor A)   S3 (Vendor B)    │
                    │  HTTP API          FTP archive       │
                    └──────────────┬──────────────────────┘
                                   │ bundle.fetch()
                    ┌──────────────▼──────────────────────┐
                    │         Bundle                       │
                    │  normalized schema · version history │
                    │  transformation record · metadata    │
                    └──────────────┬──────────────────────┘
                                   │ bb.open()
                    ┌──────────────▼──────────────────────┐
Downstream          │  pandas  polars  SQL  Flight  numpy  │
consumers           └─────────────────────────────────────┘
```

Upstream details stay inside the bundle. Downstream consumers always see the same interface.

## What you get that custom ETL scripts don't

| Concern | Custom scripts | Bundlebase |
|---------|---------------|------------|
| Idempotent re-runs | Write your own dedup logic | `fetch("add")` skips known files |
| Schema normalization | Code in the script | Committed into the bundle definition |
| Audit log | Log files (if they exist) | Commit history on the bundle |
| Downstream interface | Changes when upstream changes | Always `bb.open()` |
| Adding a new source | New script, new pipeline | Add a source, extend the bundle |
| Consumer stack lock-in | Consumers depend on your script's output format | pandas, polars, SQL, SQL |

## Scheduling the fetch

```bash
# crontab — run every day at 6am
0 6 * * * /usr/bin/python3 /opt/pipeline/scripts/fetch_vendor_sales.py >> /var/log/vendor-sales.log 2>&1
```

Or in whatever orchestrator you already use — the script is simple enough that it doesn't need a framework.

## Next steps

- [Sources](../guide/sources.md) — full connector reference: SFTP, S3 remote_dir, FTP, HTTP, Kaggle, custom
- [Data Engineer use case](data-engineer.md) — multi-source pipelines with multiple fetch strategies
- [Backend Developer use case](backend-developer.md) — publishing API data the same way
- [Versioning](../guide/versioning.md) — reset, undo, and auditing what changed
