
## Joining Data

You are able create a bundle built up of data in different files joined together. Even files of different types.

When joining, you must specify a unique `name` which will be used for disambiguation and also if you ever need to attach more data to the joined data.

You can also optionally specify a `how` which can be "inner", "left", "right", or "full"

=== "Async API"

    ```python
    import bundlebase as bb

    # Start with customers
    c = await (bb.create("my/data")
        .attach("customers.parquet"))

    # Join with orders
    await c.join("orders", "s3://external/orders.parquet", "customer_id=id")
    await c.commit("Joined orders")

    await c.to_pandas()
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    # Start with customers
    c = (bb.create("my/data")
        .attach("customers.parquet"))

    # Join with orders
    c.join("orders", "s3://external/orders.parquet", "customer_id=id")
    c.commit("Joined orders")

    c.to_pandas()
    ```

!!! note

    Until you commit, the join will not be used when the bundle is reopened. 