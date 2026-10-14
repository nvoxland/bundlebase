---
title: Bundlebase for Scheduled Collection — Recurring Fetches With an Audit Trail
description: Pull from SFTP, S3, HTTP APIs, or databases on a schedule. Define sources and cleanup rules once in the bundle; the daily job is three lines; the commit history records every run and every file. Downstream readers just open the path.
seo_title: "Bundlebase for Scheduled Collection — Recurring Fetches With an Audit Trail"
---

# Use Case: Scheduled Collection

## The problem

A vendor drops CSVs on an SFTP server every night. A partner writes Parquet to an S3 bucket. An internal API publishes reference data weekly. You write a script to fetch, clean, and load each one, and it works until the vendor renames a column, the credentials rotate, or a second vendor arrives with a slightly different format. Then the logic sprawls across scripts, and when a run fails at 3am nobody can say exactly what was loaded.

With a bundle, the sources and the cleanup rules are defined once and stored in the bundle itself. The recurring job only says "fetch and commit." Fetches are idempotent, every run is a commit, and downstream readers open one path without knowing anything about the upstream plumbing.

Bundlebase is not a scheduler. Run the job from cron, GitHub Actions, Airflow, or whatever you already have.

## Define the bundle once

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = (bb.create("s3://team-data/vendor-sales")
        .set_name("Vendor Sales — Daily Feed")
        .set_description("Daily CSVs from Vendor A via SFTP, completed transactions only.")
        .create_source("remote_dir", {
            "url": "sftp://pipeline-user@vendor.example.com/outbound/sales/",
            "patterns": "sales_*.csv",
            "key_path": "/etc/keys/vendor_rsa"
        })
        .normalize_column_names()
        .filter("SELECT * FROM bundle WHERE status = 'completed'")
        .drop_column("internal_vendor_code"))

    bundle.fetch("base", "add")
    bundle.commit("Initial load — all historical files")
    ```

=== "SQL"

    ```bash
    bundlebase repl --bundle s3://team-data/vendor-sales --create
    ```

    ```sql
    SET NAME 'Vendor Sales — Daily Feed';
    SET DESCRIPTION 'Daily CSVs from Vendor A via SFTP, completed transactions only.';
    CREATE SOURCE FOR base USING remote_dir WITH (
        url = 'sftp://pipeline-user@vendor.example.com/outbound/sales/',
        patterns = 'sales_*.csv',
        key_path = '/etc/keys/vendor_rsa'
    );
    NORMALIZE COLUMN NAMES;
    FILTER WITH SELECT * FROM bundle WHERE status = 'completed';
    DROP COLUMN internal_vendor_code;
    FETCH base ADD;
    COMMIT 'Initial load — all historical files';
    ```

The normalization, filter, and dropped column are part of the bundle's definition and apply to every file fetched from now on. If the vendor changes `Status` to `STATUS`, the downstream schema does not move.

## The recurring job

=== "Python"

    ```python
    #!/usr/bin/env python3
    import bundlebase.sync as bb
    from datetime import date

    bundle = bb.open("s3://team-data/vendor-sales").extend()
    bundle.fetch("base", "add")
    bundle.commit(f"Daily fetch — {date.today()}")
    ```

=== "SQL"

    ```bash
    bundlebase repl --bundle s3://team-data/vendor-sales
    ```

    ```sql
    FETCH base ADD;
    COMMIT 'Daily fetch';
    ```

`add` skips files already attached, so running the job twice changes nothing, and running it before today's file arrives is harmless. If the job fails before `commit`, nothing is written and the bundle stays at the last good run.

!!! tip "`add` vs `sync`"
    - **`add`** skips files already attached. Use it for append-only sources such as daily drops and date-partitioned buckets.
    - **`sync`** removes data no longer in the source and re-fetches what changed. Use it for reference data that changes in place, and for backfills.

## Cleanup rules that cannot be forgotten

Cleanup written in a script only runs when that script runs. `ALWAYS DELETE` and `ALWAYS UPDATE` rules are stored in the bundle and apply to every future attach or fetch, whoever or whatever runs it:

=== "Python"

    ```python
    bundle = bb.open("s3://team-data/vendor-sales").extend()
    bundle.always_delete("customer_id IS NULL")
    bundle.always_delete("account_type = 'internal_test'")
    bundle.always_update("SET region = 'EMEA' WHERE region IN ('Europe', 'EU', 'europe')")
    bundle.commit("Added cleanup rules")
    ```

=== "SQL"

    ```sql
    ALWAYS DELETE FROM bundle WHERE customer_id IS NULL;
    ALWAYS DELETE FROM bundle WHERE account_type = 'internal_test';
    ALWAYS UPDATE bundle SET region = 'EMEA' WHERE region IN ('Europe', 'EU', 'europe');
    COMMIT 'Added cleanup rules';
    ```

## More sources, same bundle

A second vendor with a different format joins the same bundle rather than starting a parallel pipeline. Reference data can come in as a joined pack, so readers get names instead of ids without doing the join themselves:

=== "Python"

    ```python
    bundle = bb.open("s3://team-data/vendor-sales").extend()

    # Vendor B unions into the same base pack
    bundle.create_source("remote_dir", {
        "url": "s3://partner-b/exports/",
        "patterns": "*.parquet"
    })

    # Product reference data from an API, joined as its own pack
    bundle.join("products", "bundle.product_id = products.id")
    bundle.create_source("http", {
        "url": "https://api.internal/products",
        "json_record_path": "items"
    }, pack="products")

    bundle.fetch("base", "add")
    bundle.fetch("products", "sync")
    bundle.commit("Added Vendor B and product reference data")
    ```

=== "SQL"

    ```sql
    CREATE SOURCE FOR base USING remote_dir WITH (url = 's3://partner-b/exports/', patterns = '*.parquet');
    JOIN 'https://api.internal/products' AS products ON bundle.product_id = products.id;
    CREATE SOURCE FOR products USING http WITH (url = 'https://api.internal/products', json_record_path = 'items');
    FETCH base ADD;
    FETCH products SYNC;
    COMMIT 'Added Vendor B and product reference data';
    ```

Readers still open `s3://team-data/vendor-sales` and get one table. They do not need to know a second vendor exists.

## The audit trail

Every run is a commit with a timestamp and message, and every attached file is recorded with its source location and a checksum:

```sql
SHOW HISTORY;
SELECT source_locations, source_versions, num_rows FROM bundle_info.blocks;
VERIFY DATA;
```

When someone asks what was loaded on a given day, or whether a file changed after it was loaded, the answer is in the bundle. You did not have to write the logging.

## Backfills

```sql
FETCH base SYNC;
COMMIT 'Backfill after vendor reissued March files';
```

If something goes wrong partway through, `RESET` discards everything since the last commit.

## What readers see

```python
import bundlebase.sync as bb

bundle = bb.open("s3://team-data/vendor-sales")
df = bundle.to_pandas()

for batch in bundle.stream_batches():   # larger-than-RAM data
    process(batch)
```

An AI assistant gets the same bundle over MCP, including the source of every file and the history of every run.

## Scheduling

=== "cron"

    ```bash
    0 6 * * * /usr/bin/python3 /opt/pipeline/fetch_vendor_sales.py >> /var/log/vendor-sales.log 2>&1
    ```

=== "GitHub Actions"

    ```yaml
    - name: Fetch vendor sales
      run: python scripts/fetch_vendor_sales.py
      env:
        AWS_ACCESS_KEY_ID: ${{ secrets.AWS_ACCESS_KEY_ID }}
        AWS_SECRET_ACCESS_KEY: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
    ```

=== "Airflow"

    ```python
    from airflow.operators.python import PythonOperator

    def fetch_vendor_sales():
        import bundlebase.sync as bb
        from datetime import date
        bundle = bb.open("s3://team-data/vendor-sales").extend()
        bundle.fetch("base", "add")
        bundle.commit(f"Daily fetch — {date.today()}")

    PythonOperator(task_id="fetch_vendor_sales", python_callable=fetch_vendor_sales, dag=dag)
    ```

## Compared with hand-written scripts

| Concern | Hand-written scripts | Bundle |
|---|---|---|
| Re-running safely | Write your own dedup | `fetch` with `add` skips known files |
| Schema normalization | In the script | In the bundle definition |
| Cleanup rules | In the script, if it runs | In the bundle, always applied |
| Audit log | Whatever you logged | Commit history and file checksums |
| Failed run | Find and remove the partial load | Nothing was written |
| Adding a source | New script | Add a source to the bundle |
| Reader interface | Changes with upstream | One path |

## Next steps

- [Sources](../guide/sources.md) — S3, HTTP, SFTP, FTP, Kaggle, PostgreSQL, and custom connectors
- [Versioning](../guide/versioning.md) — history, undo, reset, and data verification
- [Performance](../guide/performance.md) — streaming large datasets
- [Custom Connectors](../guide/custom-connectors/index.md) — sources with no built-in support
