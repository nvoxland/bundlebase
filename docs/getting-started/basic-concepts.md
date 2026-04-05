---
title: Basic Concepts — Bundlebase
description: "Core Bundlebase concepts: what a bundle is, read-only vs mutable, lazy evaluation, supported data formats, and the Git-like commit and versioning system."
---

# Basic Concepts

## What is a bundle?

A bundle is:

- Data from one or more sources (CSV, Parquet, JSON, HTTP APIs, cloud storage, SFTP)
- The transformations applied to it — filters, column drops, renames, joins — recorded as operations
- A versioned snapshot committed to a path that anyone can open

The key property is that **the data carries its own context**. A bundle knows its name, schema, transformation history, and where the data came from. A plain CSV or Parquet file knows none of that. You don't need a README alongside it, a schema document, or the person who made it — opening the bundle gives you everything.

=== "Python"

    ```python
    import bundlebase.sync as bb

    # Define what the data is and how it's shaped
    bundle = (bb.create("s3://my-bucket/my-bundle")
        .attach("data.parquet")
        .filter("age >= 18")
        .drop_column("ssn")
        .commit("Adults only, PII removed"))

    # Anyone opens it the same way
    bundle = bb.open("s3://my-bucket/my-bundle")
    df = bundle.to_pandas()
    ```

=== "SQL"

    ```sql
    CREATE 's3://my-bucket/my-bundle';
    ATTACH 'data.parquet';
    FILTER WITH SELECT * FROM bundle WHERE age >= 18;
    DROP COLUMN ssn;
    COMMIT 'Adults only, PII removed';

    -- Anyone opens it the same way
    OPEN 's3://my-bucket/my-bundle';
    SELECT * FROM bundle;
    ```

!!! tip "If you know Docker"
    The mental model is similar: create, attach, filter, commit is the recipe — `commit` is the push, `open` is the pull. The artifact carries its own context so consumers don't need to know how it was built.

## Read-only vs mutable

**Opening** an existing bundle gives you a **read-only** view. You can query and inspect the data, but not change it.

**Creating** a new bundle or **extending** an existing one gives you a **mutable** bundle you can attach data to, transform, and commit.

=== "Python"

    ```python
    import bundlebase.sync as bb

    # open() → read-only
    bundle = bb.open("s3://my-bucket/my-bundle")
    df = bundle.to_pandas()       # query: yes
    schema = bundle.schema        # inspect: yes

    # extend() → mutable copy, ready to modify
    bundle = bundle.extend()
    bundle.filter("active = true")
    bundle.commit("Active users only")

    # create() → new mutable bundle
    bundle = bb.create("s3://my-bucket/new-bundle")
    bundle.attach("data.parquet")
    bundle.commit("Initial load")
    ```

=== "SQL"

    ```sql
    -- OPEN → read-only
    OPEN 's3://my-bucket/my-bundle';
    SELECT * FROM bundle;    -- query: yes
    SHOW STATUS;             -- inspect: yes

    -- EXTEND → mutable, ready to modify
    EXTEND;
    FILTER WITH SELECT * FROM bundle WHERE active = true;
    COMMIT 'Active users only';

    -- CREATE → new mutable bundle
    CREATE 's3://my-bucket/new-bundle';
    ATTACH 'data.parquet';
    COMMIT 'Initial load';
    ```

## Lazy evaluation

Operations are **recorded** when you write them, but not **executed** until you actually need the data. The full pipeline runs at once when you query or export.

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("s3://my-bucket/my-bundle")
    bundle.attach("data.parquet")   # recorded
    bundle.filter("age >= 18")      # recorded
    bundle.drop_column("ssn")       # recorded

    df = bundle.to_pandas()         # pipeline executes here
    ```

=== "SQL"

    ```sql
    CREATE 's3://my-bucket/my-bundle';
    ATTACH 'data.parquet';                             -- recorded
    FILTER WITH SELECT * FROM bundle WHERE age >= 18;  -- recorded
    DROP COLUMN ssn;                                   -- recorded

    SELECT * FROM bundle;                              -- pipeline executes here
    ```

This lets Bundlebase optimize the entire pipeline before touching any data — pushing filters down to the source, skipping columns that aren't needed, and streaming results rather than loading everything into memory at once.

## Data sources and formats

There are two ways to bring data into a bundle: **attach** and **source**.

### Attach — specific files

`attach` pulls in a specific file or URL right now. Multiple attaches union together automatically, even across formats:

=== "Python"

    ```python
    bundle.attach("local.parquet")
    bundle.attach("s3://bucket/data.csv")
    bundle.attach("https://example.com/feed.json")
    bundle.attach("january.parquet")
    bundle.attach("february.parquet")   # unioned with january
    ```

=== "SQL"

    ```sql
    ATTACH 'local.parquet';
    ATTACH 's3://bucket/data.csv';
    ATTACH 'https://example.com/feed.json';
    ATTACH 'january.parquet';
    ATTACH 'february.parquet';   -- unioned with january
    ```

### Source — automatic discovery

`create_source` defines a persistent watched location (an S3 prefix, a directory, a connector endpoint). Once defined, `fetch` discovers and attaches whatever files are new — no need to track which files you've already seen.

=== "Python"

    ```python
    # Define once — watch this S3 prefix for Parquet files
    bundle.create_source("monthly_reports", {"url": "s3://exports/reports/", "patterns": "**/*.parquet"})

    # Each month: fetch picks up only the new files
    bundle.fetch("monthly_reports")
    bundle.commit("Added new monthly reports")
    ```

=== "SQL"

    ```sql
    -- Define once
    CREATE SOURCE FOR monthly_reports USING s3_connector WITH (url = 's3://exports/reports/', patterns = '**/*.parquet');

    -- Each month: fetch picks up only the new files
    FETCH monthly_reports ADD;
    COMMIT 'Added new monthly reports';
    ```

Use `attach` when you know exactly what file you want. Use `source` when you want a bundle to stay in sync with a location over time — vendor drops, recurring exports, live data directories.

Supported built-in sources: local files, S3, GCS, Azure Blob, HTTP/HTTPS, SFTP. Custom connectors can extend this to any source — see [Extending Bundlebase](why-bundlebase.md#extending-bundlebase).

## Versioning and commits

Every `commit` saves a named snapshot. The full history is stored in the bundle — no external tracking needed.

=== "Python"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("s3://my-bucket/sales")
    bundle.attach("jan.csv")
    bundle.commit("January data")

    bundle = bundle.extend()
    bundle.attach("feb.csv")
    bundle.commit("Added February")

    # View the history
    bundle = bb.open("s3://my-bucket/sales")
    for entry in bundle.history():
        print(entry)
    ```

=== "SQL"

    ```sql
    CREATE 's3://my-bucket/sales';
    ATTACH 'jan.csv';
    COMMIT 'January data';

    EXTEND;
    ATTACH 'feb.csv';
    COMMIT 'Added February';

    -- View the history
    OPEN 's3://my-bucket/sales';
    SHOW HISTORY;
    ```

## Indexes

Indexes enable fast lookups on specific columns without scanning the full dataset:

=== "Python"

    ```python
    bundle.create_index("email")

    # Queries on email now use the index
    result = bundle.query("SELECT * FROM bundle WHERE email = 'user@example.com'")
    ```

=== "SQL"

    ```sql
    CREATE INDEX email;

    -- Queries on email now use the index
    SELECT * FROM bundle WHERE email = 'user@example.com';
    ```

Indexes work regardless of the underlying storage format — including CSV. CSV columns are stored as text, but you can index them directly and equality, range, and `IN` lookups will use the index without any casting. For line-oriented formats like CSV and JSON, Bundlebase uses byte-offset reads to fetch only the matching rows rather than scanning the file.

Indexes are built lazily and Bundlebase uses cost-based optimization to decide when to use them. Learn more in the [Indexing Guide](../guide/indexing.md).
