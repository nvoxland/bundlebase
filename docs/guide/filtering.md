# Filtering

The `filter()` method narrows down the rows in a bundle using a SQL WHERE clause. Unlike `query()` which returns a separate result set, `filter()` modifies the bundle in place.

## Basic Usage

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.create("my/data")
    await bundle.attach("customers.csv")
    await bundle.filter("age >= 21")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data")
    bundle.attach("customers.csv")
    bundle.filter("age >= 21")
    ```

=== "SQL"

    ```sql
    FILTER WITH SELECT * FROM bundle WHERE age >= 21
    ```

## Parameterized Filters

Use `$1`, `$2`, etc. as placeholders to safely pass parameters:

=== "Async API"

    ```python
    await bundle.filter("salary > $1 AND department = $2", params=[50000.0, "Engineering"])
    ```

=== "Sync API"

    ```python
    bundle.filter("salary > $1 AND department = $2", params=[50000.0, "Engineering"])
    ```

!!! note
    `filter()` modifies the bundle in place and returns self for chaining. To run a query that returns an independent result set, use [`query()`](querying.md) instead.
