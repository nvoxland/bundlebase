# Joins

You can create a bundle built up of data in different files joined together, even files of different types.

When joining, you must specify a unique `name` for the join and an `on` expression defining how the data relates.

You can optionally specify a `location` for the initial data and a `how` for the join type (`"inner"`, `"left"`, `"right"`, or `"full"`).

## Basic Usage

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.create("my/data")
    await bundle.attach("customers.parquet")

    # Join with orders data
    await bundle.join("orders", on="customer_id = orders.id",
                 location="s3://external/orders.parquet")
    print(await bundle.to_pandas())
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data")
    bundle.attach("customers.parquet")

    # Join with orders data
    bundle.join("orders", on="customer_id = orders.id",
           location="s3://external/orders.parquet")

    print(bundle.to_pandas())
    ```

=== "SQL"

    ```sql
    JOIN 'orders.parquet' AS orders ON customer_id = orders.id
    ```

## Joining With Another Bundle

You can join with the query output of another committed bundle using a `bundle://` URL. This reads the target bundle's full query output — including any filters, column operations, and joins that have been applied.

**For filesystem bundles**, use `bundle://` followed by the path:

=== "Async API"

    ```python
    await bundle.join("other", on="id = other.id",
                 location="bundle:///path/to/other/bundle")
    ```

=== "Sync API"

    ```python
    bundle.join("other", on="id = other.id",
           location="bundle:///path/to/other/bundle")
    ```

**For remote bundles** (S3, etc.), use the compound scheme `bundle+<scheme>://`:

=== "Async API"

    ```python
    await bundle.join("other", on="id = other.id",
                 location="bundle+s3://bucket/path/to/bundle")
    ```

=== "Sync API"

    ```python
    bundle.join("other", on="id = other.id",
           location="bundle+s3://bucket/path/to/bundle")
    ```

!!! note
    The target bundle must be committed. The joined data reflects the target's full query output at read time, not just its raw files.

## Join Without Initial Data

Create a join point without initial data. Data can be attached later using `attach()` with the `pack` parameter or via [sources](sources.md).

=== "Async API"

    ```python
    await bundle.join("orders", on="customer_id = orders.id")

    # Attach data to the join later
    await bundle.attach("orders.parquet", pack="orders")
    ```

=== "Sync API"

    ```python
    bundle.join("orders", on="customer_id = orders.id")

    # Attach data to the join later
    bundle.attach("orders.parquet", pack="orders")
    ```

=== "SQL"

    ```sql
    JOIN AS orders ON customer_id = orders.id
    ```

## Join Types

=== "Async API"

    ```python
    # Left join (keep all customers, even without orders)
    await bundle.join("orders", on="customer_id = orders.id",
                 location="orders.parquet", how="left")

    # Full outer join
    await bundle.join("all_data", on="id = all_data.id",
                 location="other.parquet", how="full")
    ```

=== "Sync API"

    ```python
    bundle.join("orders", on="customer_id = orders.id",
           location="orders.parquet", how="left")

    bundle.join("all_data", on="id = all_data.id",
           location="other.parquet", how="full")
    ```

=== "SQL"

    ```sql
    LEFT JOIN 'orders.parquet' AS orders ON customer_id = orders.id
    ```

## Drop Join

Remove an existing join from the bundle.

=== "Async API"

    ```python
    await bundle.drop_join("orders")
    ```

=== "Sync API"

    ```python
    bundle.drop_join("orders")
    ```

=== "SQL"

    ```sql
    DROP JOIN orders
    ```

## Rename Join

Rename an existing join.

=== "Async API"

    ```python
    await bundle.rename_join("orders", new_name="customer_orders")
    ```

=== "Sync API"

    ```python
    bundle.rename_join("orders", new_name="customer_orders")
    ```

=== "SQL"

    ```sql
    RENAME JOIN orders TO customer_orders
    ```

!!! note
    Until you commit, join changes will not be used when the bundle is reopened.
