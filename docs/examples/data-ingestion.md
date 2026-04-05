---
title: Data Ingestion — Bundlebase Examples
description: "Examples for attaching data from HTTP, S3, SFTP, and other sources. Source definitions, fetch modes, and multi-source pipelines."
---

# Data Ingestion

## Attach vs. source

Use **`attach`** when you know the exact file to add right now.
Use **`create_source` + `fetch`** when you want a bundle to stay in sync with a location over time — the source definition is stored in the bundle and `fetch` discovers what's new.

---

## HTTP sources

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("s3://my-bucket/api-data")

    # JSON API
    bundle.create_source("http", {
        "url": "https://api.example.com/v2/records",
        "format": "json",
        "json_record_path": "data"   # path to the records array in the response
    })
    bundle.fetch("base", "add")
    bundle.commit("Initial fetch")
    ```

=== "SQL"

    ```sql
    CREATE 's3://my-bucket/api-data';
    CREATE SOURCE FOR base USING http WITH (
        url = 'https://api.example.com/v2/records',
        format = 'json',
        json_record_path = 'data'
    );
    FETCH base ADD;
    COMMIT 'Initial fetch';
    ```

### HTTP POST with authentication

=== "Python"

    ```python
    import json

    bundle.create_source("http", {
        "url": "https://api.example.com/v2/export",
        "method": "POST",
        "body": json.dumps({"since": "2026-01-01", "format": "json"}),
        "headers": "Authorization: Bearer TOKEN\nContent-Type: application/json",
        "json_record_path": "results"
    })
    bundle.fetch("base", "add")
    ```

=== "SQL"

    ```sql
    CREATE SOURCE FOR base USING http WITH (
        url = 'https://api.example.com/v2/export',
        method = 'POST',
        body = '{"since":"2026-01-01","format":"json"}',
        headers = 'Authorization: Bearer TOKEN\nContent-Type: application/json',
        json_record_path = 'results'
    );
    FETCH base ADD;
    ```

---

## S3 sources with glob patterns

=== "Python"

    ```python
    bundle.create_source("remote_dir", {
        "url": "s3://raw-data/events/",
        "patterns": "year=*/month=*/day=*/*.parquet"
    })
    bundle.fetch("base", "add")
    ```

=== "SQL"

    ```sql
    CREATE SOURCE FOR base USING remote_dir WITH (
        url = 's3://raw-data/events/',
        patterns = 'year=*/month=*/day=*/*.parquet'
    );
    FETCH base ADD;
    ```

---

## SFTP sources

=== "Python"

    ```python
    bundle.create_source("sftp_directory", {
        "url": "sftp://vendor.example.com/outbound/",
        "patterns": "sales_*.csv",
        "username": "pipeline-user",
        "key_path": "/etc/keys/vendor_rsa"
    })
    bundle.fetch("base", "add")
    ```

=== "SQL"

    ```sql
    CREATE SOURCE FOR base USING sftp_directory WITH (
        url = 'sftp://vendor.example.com/outbound/',
        patterns = 'sales_*.csv',
        username = 'pipeline-user',
        key_path = '/etc/keys/vendor_rsa'
    );
    FETCH base ADD;
    ```

---

## Fetch modes

| Mode | What it does |
|------|-------------|
| `add` | Add files not already in the bundle — idempotent, safe to run repeatedly |
| `update` | Add new files and refresh files that have changed |
| `sync` | Full replacement — remove files no longer in the source, add new ones |

=== "Python"

    ```python
    # Append-only: daily log files, vendor drops
    bundle.fetch("base", "add")

    # Refresh changed files: mutable reference data
    bundle.fetch("ref", "update")

    # Full replacement: snapshot sources that don't support incremental
    bundle.fetch("snapshot", "sync")
    ```

=== "SQL"

    ```sql
    FETCH base ADD;
    FETCH ref UPDATE;
    FETCH snapshot SYNC;
    ```

Preview what a fetch would do before running it:

=== "Python"

    ```python
    # Dry run — shows what would change without modifying anything
    bundle.fetch("base", "add", dry_run=True)
    ```

=== "SQL"

    ```sql
    FETCH base ADD DRY RUN;
    FETCH ALL SYNC DRY RUN;
    ```

---

## Multiple named sources

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("s3://analytics/pipeline")

    # Primary event data from S3
    bundle.create_source("remote_dir", {
        "url": "s3://raw-data/events/",
        "patterns": "**/*.parquet"
    })   # defaults to name="base"

    # Reference taxonomy from internal API
    bundle.create_source("http", {
        "url": "https://api.internal/taxonomy",
        "format": "json",
        "json_record_path": "types"
    }, name="ref")

    # Vendor supplement from SFTP
    bundle.create_source("sftp_directory", {
        "url": "sftp://vendor.example.com/outbound/",
        "patterns": "*.csv",
        "username": "pipeline-user",
        "key_path": "/etc/keys/vendor_rsa"
    }, name="vendor")

    bundle.fetch("base", "add")
    bundle.fetch("ref", "sync")
    bundle.fetch("vendor", "add")
    bundle.commit("Initial pipeline load")
    ```

=== "SQL"

    ```sql
    CREATE 's3://analytics/pipeline';

    CREATE SOURCE FOR base USING remote_dir WITH (
        url = 's3://raw-data/events/',
        patterns = '**/*.parquet'
    );
    CREATE SOURCE FOR ref USING http WITH (
        url = 'https://api.internal/taxonomy',
        format = 'json',
        json_record_path = 'types'
    );
    CREATE SOURCE FOR vendor USING sftp_directory WITH (
        url = 'sftp://vendor.example.com/outbound/',
        patterns = '*.csv',
        username = 'pipeline-user',
        key_path = '/etc/keys/vendor_rsa'
    );

    FETCH base ADD;
    FETCH ref SYNC;
    FETCH vendor ADD;
    COMMIT 'Initial pipeline load';
    ```

---

## Incremental daily run

Once sources are defined, subsequent runs are just fetch + commit:

=== "Python"

    ```python
    import bundlebase.sync as bb
    from datetime import date

    bundle = bb.open("s3://analytics/pipeline").extend()
    bundle.fetch("base", "add")
    bundle.fetch("ref", "sync")
    bundle.fetch("vendor", "add")
    bundle.commit(f"Daily refresh — {date.today()}")
    ```

=== "SQL"

    ```sql
    OPEN 's3://analytics/pipeline';
    EXTEND;
    FETCH base ADD;
    FETCH ref SYNC;
    FETCH vendor ADD;
    COMMIT 'Daily refresh';
    ```
