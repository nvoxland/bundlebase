# Querying

Bundles can be queried using SQL ([Apache DataFusion syntax](https://datafusion.apache.org/user-guide/sql/index.html)) using the `query()` method.

When querying, the table name of the bundle is `bundle`.

The `query()` method returns a `QueryResult` with methods to convert the results to your preferred format.

## Basic Usage

=== "Async API"

    ```python
    rs = await bundle.query("SELECT * FROM bundle WHERE age >= $1", params=[min_age])
    print(await rs.to_polars())
    ```

=== "Sync API"

    ```python
    rs = bundle.query("SELECT * FROM bundle WHERE age >= $1", params=[min_age])
    print(rs.to_polars())
    ```

=== "SQL"

    ```sql
    SELECT * FROM bundle WHERE age >= 21
    ```

=== "REPL"

    ```
    /show
    ```

## Result Formats

The `QueryResult` object supports several output formats:

=== "Async API"

    ```python
    rs = await bundle.query("SELECT * FROM bundle")

    df = await rs.to_pandas()     # pandas DataFrame
    df = await rs.to_polars()     # Polars DataFrame
    data = await rs.to_dict()     # Dictionary of lists
    ```

=== "Sync API"

    ```python
    rs = bundle.query("SELECT * FROM bundle")

    df = rs.to_pandas()     # pandas DataFrame
    df = rs.to_polars()     # Polars DataFrame
    data = rs.to_dict()     # Dictionary of lists
    ```

For large datasets, stream results batch by batch:

=== "Async API"

    ```python
    rs = await bundle.query("SELECT * FROM bundle")
    async for batch in rs.stream_batches():
        process(batch)
    ```

## Parameterized Queries

Use `$1`, `$2`, etc. as placeholders to safely pass parameters:

=== "Async API"

    ```python
    rs = await bundle.query(
        "SELECT name, salary FROM bundle WHERE salary > $1 AND department = $2",
        params=[50000.0, "Engineering"]
    )
    ```

=== "Sync API"

    ```python
    rs = bundle.query(
        "SELECT name, salary FROM bundle WHERE salary > $1 AND department = $2",
        params=[50000.0, "Engineering"]
    )
    ```

## Explain Plan

Use `explain()` to see the query execution plan without running the query:

=== "Async API"

    ```python
    plan = await bundle.explain()
    print(plan)
    ```

=== "Sync API"

    ```python
    plan = bundle.explain()
    print(plan)
    ```

=== "SQL"

    ```sql
    EXPLAIN PLAN
    ```
