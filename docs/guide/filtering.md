---
seo_title: "Filtering Rows and Persistent Cleanup Rules - Bundlebase"
---

# Filtering

The `filter()` method narrows down the rows in a bundle using a SQL SELECT query. Unlike `query()` which returns a separate result set, `filter()` modifies the bundle in place.

## Basic Usage

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.create("my/data")
    await bundle.attach("customers.csv")
    await bundle.filter("SELECT * FROM bundle WHERE CAST(age AS INT) >= 21")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data")
    bundle.attach("customers.csv")
    bundle.filter("SELECT * FROM bundle WHERE CAST(age AS INT) >= 21")
    ```

=== "SQL"

    ```sql
    FILTER WITH SELECT * FROM bundle WHERE CAST(age AS INT) >= 21
    ```

## Parameterized Filters

Use `$1`, `$2`, etc. as placeholders to safely pass parameters:

=== "Async API"

    ```python
    await bundle.filter("SELECT * FROM bundle WHERE CAST(salary AS DOUBLE) > $1 AND department = $2", params=[50000.0, "Engineering"])
    ```

=== "Sync API"

    ```python
    bundle.filter("SELECT * FROM bundle WHERE CAST(salary AS DOUBLE) > $1 AND department = $2", params=[50000.0, "Engineering"])
    ```

!!! note
    CSV, TSV, and JSONL columns are imported as text. Comparing one to a number with `>`, `>=`, `<`, `<=`, or `BETWEEN` is rejected (as text it would compare lexicographically, so `"9" > "50"`). `CAST` the column as above, or convert it once with `CAST COLUMN`.

!!! note
    `filter()` modifies the bundle in place and returns self for chaining. To run a query that returns an independent result set, use [`query()`](querying.md) instead.
