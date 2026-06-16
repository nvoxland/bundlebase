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

    ```bash
    bundlebase repl --bundle s3://my-bucket/api-data --create
    ```

    ```sql
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
    bundle.create_source("remote_dir", {
        "url": "sftp://pipeline-user@vendor.example.com/outbound/",
        "patterns": "sales_*.csv",
        "key_path": "/etc/keys/vendor_rsa"
    })
    bundle.fetch("base", "add")
    ```

=== "SQL"

    ```sql
    CREATE SOURCE FOR base USING remote_dir WITH (
        url = 'sftp://pipeline-user@vendor.example.com/outbound/',
        patterns = 'sales_*.csv',
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

Preview what a fetch would do before running it (SQL only -- the Python `fetch()` does not support dry runs yet):

```sql
FETCH base ADD DRY RUN;
FETCH ALL SYNC DRY RUN;
```

---

## Multiple sources

A source belongs to a pack: either the base pack or a joined pack (created with `join`). Sources on the same pack union together; a join gets its own pack with its own sources.

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("s3://analytics/pipeline")

    # Primary event data from S3 (base pack)
    bundle.create_source("remote_dir", {
        "url": "s3://raw-data/events/",
        "patterns": "**/*.parquet"
    })

    # Vendor supplement from SFTP unions into the same base pack
    bundle.create_source("remote_dir", {
        "url": "sftp://pipeline-user@vendor.example.com/outbound/",
        "patterns": "*.csv",
        "key_path": "/etc/keys/vendor_rsa"
    })

    # Reference taxonomy joins as its own pack
    bundle.join("ref", on="bundle.event_type_id = ref.id")
    bundle.create_source("http", {
        "url": "https://api.internal/taxonomy",
        "format": "json",
        "json_record_path": "types"
    }, pack="ref")

    bundle.fetch("base", "add")
    bundle.fetch("ref", "sync")
    bundle.commit("Initial pipeline load")
    ```

=== "SQL"

    ```bash
    bundlebase repl --bundle s3://analytics/pipeline --create
    ```

    ```sql
    CREATE SOURCE FOR base USING remote_dir WITH (
        url = 's3://raw-data/events/',
        patterns = '**/*.parquet'
    );
    CREATE SOURCE FOR base USING remote_dir WITH (
        url = 'sftp://pipeline-user@vendor.example.com/outbound/',
        patterns = '*.csv',
        key_path = '/etc/keys/vendor_rsa'
    );

    FETCH base ADD;
    COMMIT 'Initial pipeline load';
    ```

    Creating a join point without initial data (so a joined pack can get its own source) requires the Python API's `join()`.

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
    bundle.commit(f"Daily refresh — {date.today()}")
    ```

=== "SQL"

    ```bash
    bundlebase repl --bundle s3://analytics/pipeline
    ```

    ```sql
    FETCH base ADD;
    FETCH ref SYNC;
    COMMIT 'Daily refresh';
    ```
