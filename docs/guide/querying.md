# Querying

Bundles can be queried using SQL ([Apache Datafusion syntax](https://datafusion.apache.org/user-guide/sql/index.html)) using the `select` method.

When querying, the table name of the bundle is "bundle".

The object returned from `select` is actually an independent bundle, and so besides the expected operations like `.to_pandas` and `.to_polars` to get the data out,
you can add indexes and commit the result as either as a stand-alone bundle or [as a view](views.md).

!!! tip "SELECT keyword is optional"
    The `SELECT` keyword is auto-prepended if missing, so you can write `c.select("* from bundle")` instead of `c.select("SELECT * from bundle")`.

=== "Async API"

    ```python
    rs = await c.select("select * from bundle where age >= $1", [min_age])
    print(rs.to_polars())

    # SELECT keyword is optional
    rs = await c.select("* from bundle where age >= $1", [min_age])
    ```

=== "Sync API"

    ```python
    rs = c.select("select * from bundle where age >= $1", [min_age])
    print(rs.to_polars())

    # SELECT keyword is optional
    rs = c.select("* from bundle where age >= $1", [min_age])
    ```
