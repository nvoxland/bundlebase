---
title: Bundlebase for Data Engineers — Automated Incremental Data Pipelines
description: Build idempotent data pipelines with Bundlebase sources. Fetch incrementally from S3, HTTP APIs, and SFTP. Commit history is your pipeline audit log. Works with cron, Airflow, and GitHub Actions.
---

# Use Case: Data Engineer

## The problem

Manual data pulls don't scale. If you're running the same fetch-clean-commit steps every day or week, that's a pipeline — it just doesn't have automation yet. And when it fails at 3am, you want to know what ran, what didn't, and exactly where the data stands.

Bundlebase isn't an orchestrator, but it fits cleanly into one. Define your sources once, run `fetch()` on a schedule, and commit. The bundle tracks what changed with each run, the fetch logic is idempotent (won't duplicate records already in the bundle), and the commit history is your pipeline audit log.

## The scenario

You're building a daily pipeline that pulls from three sources into a single analytics dataset:

- A cloud directory of Parquet files that grows daily (S3)
- An HTTP API for reference data (refreshed weekly)
- A vendor SFTP drop for supplementary records

All three feed into one bundle that the analytics team queries.

## Step 1: Define the bundle and its sources

You only do this once — source definitions are stored in the bundle:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = (bb.create("s3://analytics/events-pipeline")
        .set_name("Events Pipeline")
        .set_description(
            "Daily events from S3 partitions (base), "
            "reference taxonomy from API (ref), "
            "vendor supplement from SFTP (vendor). "
            "Joined on event_type_id."
        )
        # Source 1: S3 directory of daily Parquet partitions
        .create_source("remote_dir", {
            "url": "s3://raw-data/events/",
            "patterns": "year=*/month=*/day=*/*.parquet"
        })
        # Source 2: reference taxonomy API
        .create_source("http", {
            "url": "https://api.internal/taxonomy/event-types",
            "format": "json",
            "json_record_path": "types"
        }, name="ref")
        # Source 3: vendor SFTP drop
        .create_source("sftp_directory", {
            "url": "sftp://vendor.example.com/outbound/",
            "patterns": "*.csv",
            "username": "pipeline-user",
            "key_path": "/etc/keys/vendor_rsa"
        }, name="vendor")
    )

    # Initial fetch of all three sources
    bundle.fetch("base", "add")    # events — add new only
    bundle.fetch("ref", "replace") # reference data — always replace with latest
    bundle.fetch("vendor", "add")  # vendor supplement — add new only

    # Join reference data so consumers get event names, not just IDs
    bundle.join("ref", on="bundle.event_type_id = ref.id")

    bundle.commit("Pipeline initialized — first fetch")
    print(f"Initialized: {bundle.num_rows:,} rows")
    ```

=== "SQL"

    ```sql
    CREATE 's3://analytics/events-pipeline';
    SET NAME 'Events Pipeline';
    SET DESCRIPTION 'Daily events from S3 partitions (base), reference taxonomy from API (ref), vendor supplement from SFTP (vendor). Joined on event_type_id.';

    -- Source 1: S3 directory of daily Parquet partitions
    CREATE SOURCE FOR base USING remote_dir WITH (
        url = 's3://raw-data/events/',
        patterns = 'year=*/month=*/day=*/*.parquet'
    );

    -- Source 2: reference taxonomy API
    CREATE SOURCE FOR ref USING http WITH (
        url = 'https://api.internal/taxonomy/event-types',
        format = 'json',
        json_record_path = 'types'
    );

    -- Source 3: vendor SFTP drop
    CREATE SOURCE FOR vendor USING sftp_directory WITH (
        url = 'sftp://vendor.example.com/outbound/',
        patterns = '*.csv',
        username = 'pipeline-user',
        key_path = '/etc/keys/vendor_rsa'
    );

    -- Initial fetch of all three sources
    FETCH base ADD;
    FETCH ref SYNC;
    FETCH vendor ADD;

    -- Join reference data so consumers get event names, not just IDs
    JOIN 'ref' AS ref ON bundle.event_type_id = ref.id;

    COMMIT 'Pipeline initialized — first fetch';
    SHOW STATUS;
    ```

## Step 2: Daily run script

The pipeline script is simple because all the source definitions are already in the bundle:

=== "Python"

    ```python
    #!/usr/bin/env python3
    # scripts/run_pipeline.py

    import bundlebase.sync as bb
    from datetime import date

    bundle = bb.open("s3://analytics/events-pipeline").extend()

    try:
        bundle.fetch("base", "add")     # new event partitions only
        bundle.fetch("ref", "replace")  # refresh reference data
        bundle.fetch("vendor", "add")   # new vendor records only

        bundle.commit(f"Daily refresh — {date.today()}")
        print(f"Done: {bundle.num_rows:,} rows at version {bundle.version}")

    except Exception as e:
        print(f"Pipeline failed: {e}")
        raise
    ```

=== "SQL"

    ```sql
    OPEN 's3://analytics/events-pipeline';
    EXTEND;

    FETCH base ADD;
    FETCH ref SYNC;
    FETCH vendor ADD;

    COMMIT 'Daily refresh — <today>';
    SHOW STATUS;
    ```

!!! tip "`add` vs `replace`"
    - **`add`** — skips files already attached to the bundle (safe for append-only sources like S3 partitions)
    - **`replace`** — removes previously fetched data from this source and re-fetches everything (right for reference data that changes in-place)

## Step 3: Monitoring and auditing

The commit history is your pipeline log:

=== "Python"

    ```python
    bundle = bb.open("s3://analytics/events-pipeline")

    for entry in bundle.history():
        print(entry)
    # v1:  Pipeline initialized — first fetch
    # v2:  Daily refresh — 2026-03-28
    # v3:  Daily refresh — 2026-03-29
    # v4:  Daily refresh — 2026-03-30
    ```

=== "SQL"

    ```sql
    OPEN 's3://analytics/events-pipeline';
    SHOW HISTORY;
    ```

For more detail, query the bundle metadata directly:

=== "Python"

    ```python
    # How many rows were in the dataset on a specific date?
    # (If you're storing a fetched_date column in the data)
    by_day = bundle.query("""
        SELECT DATE(fetched_at) as date, COUNT(*) as rows
        FROM bundle
        GROUP BY DATE(fetched_at)
        ORDER BY date DESC
        LIMIT 14
    """).to_pandas()
    ```

=== "SQL"

    ```sql
    SELECT DATE(fetched_at) as date, COUNT(*) as rows
    FROM bundle
    GROUP BY DATE(fetched_at)
    ORDER BY date DESC
    LIMIT 14;
    ```

## Step 4: What consumers see

The analytics team doesn't care about your pipeline — they just open the bundle:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://analytics/events-pipeline")
    df = bundle.to_pandas()

    # Or stream it — events datasets can be large
    for batch in bundle.stream_batches():
        process(batch)  # handles datasets larger than RAM
    ```

=== "SQL"

    ```sql
    OPEN 's3://analytics/events-pipeline';
    SELECT * FROM bundle;
    ```

Because the reference join is committed into the bundle, consumers always get `event_type_name` alongside `event_type_id` — the join is baked in, not something each consumer has to do themselves.

## Persistent cleanup rules

Source data is dirty. If you're encoding your cleanup steps in the pipeline script, they only run when that script runs — and they don't apply when a colleague extends the bundle manually, or when a new script takes over.

`always_delete` and `always_update` rules are stored in the bundle itself. They apply to every future attach, automatically, regardless of who or what does the attaching:

=== "Python"

    ```python
    bundle = bb.open("s3://analytics/events-pipeline").extend()

    # Define once — fire on every future attach to this bundle
    bundle.always_delete("WHERE event_type = 'internal_test'")
    bundle.always_delete("WHERE user_id IS NULL")
    bundle.always_update("SET region = 'EMEA' WHERE region IN ('Europe', 'EU', 'europe')")

    bundle.commit("Added persistent cleanup rules")
    ```

=== "SQL"

    ```sql
    OPEN 's3://analytics/events-pipeline';
    EXTEND;

    ALWAYS DELETE WHERE event_type = 'internal_test';
    ALWAYS DELETE WHERE user_id IS NULL;
    ALWAYS UPDATE SET region = 'EMEA' WHERE region IN ('Europe', 'EU', 'europe');

    COMMIT 'Added persistent cleanup rules';
    ```

From this point on, every `fetch()` call — in this script or any other — applies these rules to incoming data before it's added to the bundle.

```python
# In the daily run script — no cleanup code needed, rules are already in the bundle
bundle = bb.open("s3://analytics/events-pipeline").extend()
bundle.fetch("base", "add")   # cleanup rules fire automatically
bundle.commit(f"Daily refresh — {date.today()}")
```

**`always_update`** is useful for normalizing values that vary across sources:

=== "Python"

    ```python
    bundle.always_update("SET currency = UPPER(currency)")           # normalize 'usd' -> 'USD'
    bundle.always_update("SET amount = ROUND(amount, 2)")            # normalize precision
    bundle.always_update("SET event_type = LOWER(event_type)")       # normalize casing
    ```

=== "SQL"

    ```sql
    ALWAYS UPDATE SET currency = UPPER(currency);
    ALWAYS UPDATE SET amount = ROUND(amount, 2);
    ALWAYS UPDATE SET event_type = LOWER(event_type);
    ```

Rules accumulate — you can add new ones at any point in the pipeline's life. They're stored in the manifest alongside the source definitions and are visible in the commit history when they were added.

## Handling backfills

When you need to re-process historical data:

=== "Python"

    ```python
    bundle = bb.open("s3://analytics/events-pipeline").extend()

    # Replace just the base source (events) without touching ref or vendor
    bundle.fetch("base", "replace")  # re-attach all matching partitions
    bundle.commit("Backfill: re-fetched all base partitions after schema fix")
    ```

=== "SQL"

    ```sql
    OPEN 's3://analytics/events-pipeline';
    EXTEND;

    FETCH base SYNC;
    COMMIT 'Backfill: re-fetched all base partitions after schema fix';
    ```

If something goes wrong mid-backfill, reset discards everything since the last commit:

=== "Python"

    ```python
    bundle.reset()  # back to the previous committed state, cleanly
    ```

=== "SQL"

    ```sql
    RESET;
    ```

## Plugging into an orchestrator

Bundlebase doesn't replace a scheduler — use whatever you already have:

=== "cron"

    ```bash
    # crontab
    0 5 * * * /usr/bin/python3 /opt/pipeline/scripts/run_pipeline.py >> /var/log/pipeline.log 2>&1
    ```

=== "GitHub Actions"

    ```yaml
    - name: Run pipeline
      run: poetry run python scripts/run_pipeline.py
      env:
        AWS_ACCESS_KEY_ID: ${{ secrets.AWS_ACCESS_KEY_ID }}
        AWS_SECRET_ACCESS_KEY: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
    ```

=== "Airflow"

    ```python
    from airflow.operators.python import PythonOperator

    def run_pipeline():
        import bundlebase.sync as bb
        from datetime import date
        bundle = bb.open("s3://analytics/events-pipeline").extend()
        bundle.fetch("base", "add")
        bundle.fetch("ref", "replace")
        bundle.fetch("vendor", "add")
        bundle.commit(f"Daily refresh — {date.today()}")

    refresh_task = PythonOperator(
        task_id="refresh_events_pipeline",
        python_callable=run_pipeline,
        dag=dag
    )
    ```

## Why this beats ad-hoc scripts

| Concern | Ad-hoc script | Bundlebase pipeline |
|---------|---------------|---------------------|
| Idempotent re-runs | Manual dedup logic | `fetch("add")` skips known files |
| Audit log | Log files (if you wrote them) | Commit history, always |
| Schema documentation | README (maybe) | Self-describing bundle |
| Consumer access | S3 raw files + docs | One `bb.open()` |
| Partial failure recovery | Manual cleanup | `bundle.reset()` |

## Next steps

- [Sources](../guide/sources.md) — full connector reference: S3, HTTP, SFTP, FTP, Kaggle
- [Versioning](../guide/versioning.md) — reset, undo, and history in detail
- [Performance](../guide/performance.md) — streaming execution for large datasets
- [Custom Connectors](../guide/custom-connectors/index.md) — build connectors for non-standard sources
