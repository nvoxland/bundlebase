---
title: Bundlebase for Backend Developers — Make API Data Self-Service
description: Publish your API or database data as a versioned Bundlebase bundle. Data teams get pandas DataFrames in one line. Works from Python, Go, Java, or any stack via HTTP or Parquet export.
---

# Use Case: Backend Developer

## The problem

Your service has data in it. Analysts and data scientists keep asking you to export it for them. You write a one-off script, they run it, they find an edge case, they ask again. Or you expose an internal API endpoint and they write their own export logic — and now there are three slightly different versions of "the same data" floating around.

Bundlebase lets you publish your data once in a form that anyone can consume directly from Python, without you being in the loop for every request. The data is versioned, the schema is self-documenting, and consumers get pandas DataFrames in one line.

## The scenario

You're a backend developer at a SaaS company. You have a REST API that returns customer usage metrics. The data science team wants this data weekly. You'd rather not write a custom ETL job, manage a shared database, or email CSV files.

## Option A: Use your existing API as a source (no extra code)

If your API already returns JSON or CSV, Bundlebase can pull from it directly. You just configure a source pointing at your endpoint:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = (bb.create("s3://team-data/usage-metrics")
        .set_name("Customer Usage Metrics")
        .set_description("Weekly pull from /api/v2/metrics. Covers all accounts, all event types.")
        .create_source("http", {
            "url": "https://api.yourservice.com/v2/metrics/export",
            "headers": "Authorization: Bearer YOUR_TOKEN\nAccept: application/json",
            "json_record_path": "data"
        })
        .fetch("base", "replace"))  # replace = full refresh each time

    bundle.commit("Weekly usage metrics — 2026-04-04")
    ```

=== "SQL"

    ```sql
    CREATE 's3://team-data/usage-metrics';
    SET NAME 'Customer Usage Metrics';
    SET DESCRIPTION 'Weekly pull from /api/v2/metrics. Covers all accounts, all event types.';

    CREATE SOURCE FOR base USING http WITH (
        url = 'https://api.yourservice.com/v2/metrics/export',
        headers = 'Authorization: Bearer YOUR_TOKEN\nAccept: application/json',
        json_record_path = 'data'
    );

    FETCH base SYNC;
    COMMIT 'Weekly usage metrics — 2026-04-04';
    ```

`fetch("base", "replace")` does a full refresh — it replaces the previous data with the current API response. Use `"add"` instead if your API returns only new records and you want to accumulate.

!!! note "Authentication"
    The `headers` field takes `Name: Value` pairs, one per line. For tokens that rotate, you can template this via environment variables or a wrapper script that sets the header before calling `fetch()`.

## Option B: Parameterized pulls

If your API takes date ranges or other parameters, use POST:

=== "Python"

    ```python
    import json
    from datetime import date, timedelta

    week_ago = (date.today() - timedelta(days=7)).isoformat()

    bundle = bb.open("s3://team-data/usage-metrics").extend()
    bundle.create_source("http", {
        "url": "https://api.yourservice.com/v2/metrics/export",
        "method": "POST",
        "body": json.dumps({"since": week_ago, "format": "json"}),
        "headers": "Authorization: Bearer YOUR_TOKEN\nContent-Type: application/json",
        "json_record_path": "results"
    })
    bundle.fetch("base", "add")
    bundle.commit(f"Usage metrics week of {week_ago}")
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/usage-metrics';
    EXTEND;

    CREATE SOURCE FOR base USING http WITH (
        url = 'https://api.yourservice.com/v2/metrics/export',
        method = 'POST',
        body = '{"since": "<week_ago>", "format": "json"}',
        headers = 'Authorization: Bearer YOUR_TOKEN\nContent-Type: application/json',
        json_record_path = 'results'
    );

    FETCH base ADD;
    COMMIT 'Usage metrics week of <week_ago>';
    ```

## Option C: Export from your database (for non-Python stacks)

If you're running Go, Java, Node, or anything else, you don't need the Python API at all to publish data. Export to Parquet or CSV and write it to a location Bundlebase can read:

```go
// Go example: write a Parquet file to S3, then let Bundlebase pick it up
rows := db.Query("SELECT account_id, event_type, count, ts FROM usage WHERE ts > ?", lastWeek)
writeParquetToS3(rows, "s3://team-data/raw/usage-2026-04-04.parquet")
```

Then a small Python script (or cron job) publishes it as a bundle:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://team-data/usage-metrics").extend()
    bundle.attach("s3://team-data/raw/usage-2026-04-04.parquet")
    bundle.commit("Usage metrics week of 2026-03-28")
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/usage-metrics';
    EXTEND;

    ATTACH 's3://team-data/raw/usage-2026-04-04.parquet';
    COMMIT 'Usage metrics week of 2026-03-28';
    ```

The data team gets a consistently structured, versioned dataset. You control the export format from your side, they consume it from theirs.

## What the data team gets

Once you've committed the bundle, the data science team consumes it without involving you:

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.open("s3://team-data/usage-metrics")

    # See what's in it before loading
    print(bundle.name)           # "Customer Usage Metrics"
    print(bundle.description)    # what it covers, how it's sourced
    print(bundle.num_rows)       # current row count
    print(bundle.version)        # which commit they're on

    # Pull into pandas
    df = bundle.to_pandas()

    # Or query directly — no need to load 10M rows to answer one question
    top_accounts = bundle.query("""
        SELECT account_id, SUM(count) as total_events
        FROM bundle
        WHERE event_type = 'api_call'
        GROUP BY account_id
        ORDER BY total_events DESC
        LIMIT 20
    """).to_pandas()
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/usage-metrics';
    SHOW STATUS;
    SHOW SCHEMA;

    SELECT * FROM bundle;

    SELECT account_id, SUM(count) as total_events
    FROM bundle
    WHERE event_type = 'api_call'
    GROUP BY account_id
    ORDER BY total_events DESC
    LIMIT 20;
    ```

They can also check what changed between updates:

=== "Python"

    ```python
    for entry in bundle.history():
        print(entry)
    # v1: Weekly usage metrics — 2026-03-28
    # v2: Weekly usage metrics — 2026-04-04
    ```

=== "SQL"

    ```sql
    SHOW HISTORY;
    ```

## Automating the weekly publish

Wrap the publish step in a script and run it from a cron job or CI pipeline:

=== "Python"

    ```python
    #!/usr/bin/env python3
    # scripts/publish_usage_bundle.py

    import bundlebase.sync as bb
    from datetime import date

    bundle = bb.open("s3://team-data/usage-metrics").extend()
    bundle.create_source("http", {
        "url": "https://api.yourservice.com/v2/metrics/export",
        "headers": f"Authorization: Bearer {API_TOKEN}"
    })
    bundle.fetch("base", "replace")
    bundle.commit(f"Weekly usage metrics — {date.today()}")

    print(f"Published: {bundle.num_rows:,} rows at version {bundle.version}")
    ```

=== "SQL"

    ```sql
    OPEN 's3://team-data/usage-metrics';
    EXTEND;

    CREATE SOURCE FOR base USING http WITH (
        url = 'https://api.yourservice.com/v2/metrics/export',
        headers = 'Authorization: Bearer <API_TOKEN>'
    );

    FETCH base SYNC;
    COMMIT 'Weekly usage metrics — <today>';
    SHOW STATUS;
    ```

```bash
# crontab
0 6 * * 1 python3 scripts/publish_usage_bundle.py
```

## Option D: Expose the bundle as a SQL server

If your consumers don't use Python at all — R, Julia, Metabase, DBeaver, any JDBC/ODBC tool — you can run a SQL server directly on top of the bundle with one command:

```bash
bundlebase serve --bundle s3://team-data/usage-metrics --port 32010
```

That's it. Any SQL client connects to `localhost:32010` and queries `bundle` as a table. The data is read-only — consumers can't change what's committed.

**From R:**
```r
library(arrow)
conn <- flight_connect("localhost", 32010)
df <- flight_get(conn, "SELECT account_id, SUM(count) as events FROM bundle GROUP BY account_id")
```

**From Metabase, DBeaver, or any JDBC tool:**
Use the [Arrow Flight SQL JDBC driver](https://central.sonatype.com/artifact/org.apache.arrow/flight-sql-jdbc-driver). Connection URL: `jdbc:arrow-flight-sql://localhost:32010`. Query `bundle` as a table — same SQL syntax as everywhere else.

**From Go:**
```go
// Standard SQL client library
client, _ := flightsql.NewClient("localhost:32010", nil, nil, grpc.WithTransportCredentials(insecure.NewCredentials()))
info, _ := client.Execute(ctx, "SELECT * FROM bundle WHERE event_type = 'api_call' LIMIT 1000")
```

The bundle on S3 doesn't move. You run the server where the data lives — on a bastion host with S3 access, in a Docker container, in a GitHub Actions job — and point consumers at the port. No ETL, no data duplication, no new infrastructure to maintain.

## Why this beats "just expose a CSV endpoint"

| Approach | Schema docs | Version history | Queryable | Self-service |
|----------|-------------|-----------------|-----------|--------------|
| Email CSV | No | No | No | No |
| CSV endpoint | No | No | Requires pandas | Barely |
| Shared database | Requires setup | Depends | Yes | Requires credentials |
| Bundlebase | Yes | Yes | Yes | `bb.open(path)` |

## Next steps

- [Sources](../guide/sources.md) — full HTTP connector options including POST/PUT and auth headers
- [Versioning](../guide/versioning.md) — commit history and how consumers track updates
- [Custom Connectors](../guide/custom-connectors/index.md) — if you want deeper integration from Go, Java, or Rust
