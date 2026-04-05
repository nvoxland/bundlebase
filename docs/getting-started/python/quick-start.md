---
title: Python Quick Start — Bundlebase
description: Get started with the Bundlebase Python API. Attach Parquet, CSV, or JSON from local files or S3, filter and transform, commit, and query with SQL.
---

# Python Quick Start

## Choose your API style

Bundlebase has two Python API styles:

- **Sync** (`bundlebase.sync`) — for scripts and Jupyter notebooks. No `await` needed.
- **Async** (`bundlebase`) — for concurrent operations and production code.

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    c = bb.create("s3://mybucket/path")
    c.attach("data.parquet")
    df = c.to_pandas()
    ```

=== "Async API"

    ```python
    import bundlebase as bb

    c = await bb.create("s3://mybucket/path")
    await c.attach("data.parquet")
    df = await c.to_pandas()
    ```

## Create a bundle

The path can be a local filepath or a remote URL (S3, Azure, GCS):

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    c = bb.create("s3://mybucket/sales-q1")
    ```

=== "Async API"

    ```python
    import bundlebase as bb

    c = await bb.create("s3://mybucket/sales-q1")
    ```

## Attach data

Parquet, CSV, and JSON are all supported. Attaching multiple files unions them together, even across formats. Paths can be relative to the bundle or absolute URLs.

=== "Sync API"

    ```python
    c.attach("local_data.parquet")
    c.attach("s3://other_bucket/more_data.csv")
    c.attach("https://example.com/additional.json")
    ```

=== "Async API"

    ```python
    await c.attach("local_data.parquet")
    await c.attach("s3://other_bucket/more_data.csv")
    await c.attach("https://example.com/additional.json")
    ```

!!! note
    CSV columns are imported as text. Use `cast_column()` to convert to integer, float, etc. See [Column Types](../../guide/attaching.md#column-types) for details.

## Transform

=== "Sync API"

    ```python
    c.filter("age >= 18")
    c.drop_column("ssn")
    c.rename_column("fname", "first_name")
    ```

=== "Async API"

    ```python
    await c.filter("age >= 18")
    await c.drop_column("ssn")
    await c.rename_column("fname", "first_name")
    ```

## Commit

=== "Sync API"

    ```python
    c.commit("Initial commit")

    # Anyone with the path can open it
    c = bb.open("s3://mybucket/sales-q1")
    ```

=== "Async API"

    ```python
    await c.commit("Initial commit")

    c = await bb.open("s3://mybucket/sales-q1")
    ```

## Query with SQL

Full [Apache DataFusion SQL syntax](https://datafusion.apache.org/user-guide/sql/index.html):

=== "Sync API"

    ```python
    rs = c.query("SELECT * FROM bundle WHERE revenue > 100")
    df = rs.to_polars()
    ```

=== "Async API"

    ```python
    rs = await c.query("SELECT * FROM bundle WHERE revenue > 100")
    df = await rs.to_polars()
    ```

## Export

=== "Sync API"

    ```python
    df = c.to_pandas()
    df = c.to_polars()
    arrays = c.to_numpy()
    ```

=== "Async API"

    ```python
    df = await c.to_pandas()
    df = await c.to_polars()
    arrays = await c.to_numpy()
    ```

## Method chaining

All mutation methods return `self`:

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    c = (bb.create("s3://mybucket/sales-q1")
        .attach("data.parquet")
        .filter("active = true")
        .drop_column("temp_field")
        .commit("Initial commit"))
    ```

=== "Async API"

    ```python
    import bundlebase as bb

    c = await (bb.create("s3://mybucket/sales-q1")
        .attach("data.parquet")
        .filter("active = true")
        .drop_column("temp_field")
        .commit("Initial commit"))
    ```

## Next steps

- [Basic Concepts](../basic-concepts.md) — bundles, operations, and versioning
- [User Guide](../../guide/attaching.md) — deep dive into advanced topics
- [API Reference](../../api/python/index.md) — complete API documentation
