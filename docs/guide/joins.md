---
seo_title: "Joining Bundles and External Data - Bundlebase"
---

# Joins

You can create a bundle built up of data in different files joined together, even files of different types.

When joining, you must specify a unique `name` for the join and an `on` expression defining how the data relates. Use `bundle.` to reference columns from the bundle's existing data in the join expression.

You can optionally specify a `location` for the initial data and a join type (`"inner"`, `"left"`, `"right"`, or `"full"`). The join type parameter is named `how` in the sync API and `join_type` in the async API.

## Basic Usage

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.create("my/data")
    await bundle.attach("customers.parquet")

    # Join with orders data
    await bundle.join("orders", "bundle.customer_id = orders.id",
                 location="s3://external/orders.parquet")
    print(await bundle.to_pandas())
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data")
    bundle.attach("customers.parquet")

    # Join with orders data
    bundle.join("orders", "bundle.customer_id = orders.id",
           location="s3://external/orders.parquet")

    print(bundle.to_pandas())
    ```

=== "SQL"

    ```sql
    JOIN 'orders.parquet' AS orders ON bundle.customer_id = orders.id
    ```

## Joining With Another Bundle

You can join with the query output of another committed bundle using a `bundle://` URL. This reads the target bundle's full query output, including any filters, column operations, and joins that have been applied.

**For filesystem bundles**, use `bundle://` followed by the path:

=== "SQL"

    ```sql
    JOIN 'bundle:///path/to/other/bundle' AS other ON bundle.id = other.id
    ```

=== "Async API"

    ```python
    await bundle.join("other", "bundle.id = other.id",
                 location="bundle:///path/to/other/bundle")
    ```

=== "Sync API"

    ```python
    bundle.join("other", "bundle.id = other.id",
           location="bundle:///path/to/other/bundle")
    ```

**For remote bundles** (S3, etc.), use the compound scheme `bundle+<scheme>://`:

=== "SQL"

    ```sql
    JOIN 'bundle+s3://bucket/path/to/bundle' AS other ON bundle.id = other.id
    ```

=== "Async API"

    ```python
    await bundle.join("other", "bundle.id = other.id",
                 location="bundle+s3://bucket/path/to/bundle")
    ```

=== "Sync API"

    ```python
    bundle.join("other", "bundle.id = other.id",
           location="bundle+s3://bucket/path/to/bundle")
    ```

!!! note
    The target bundle must be committed. The joined data reflects the target's full query output at read time, including any filters, column operations, and joins that have been applied.

## Join Without Initial Data

Create a join point without initial data. Data can be attached later using `attach()` with the `pack` parameter or via [sources](sources.md).

=== "Async API"

    ```python
    await bundle.join("orders", "bundle.customer_id = orders.id")

    # Attach data to the join later
    await bundle.attach("orders.parquet", pack="orders")
    ```

=== "Sync API"

    ```python
    bundle.join("orders", "bundle.customer_id = orders.id")

    # Attach data to the join later
    bundle.attach("orders.parquet", pack="orders")
    ```

## Join Types

=== "Async API"

    ```python
    # Left join (keep all customers, even without orders)
    await bundle.join("orders", "bundle.customer_id = orders.id",
                 location="orders.parquet", join_type="left")

    # Full outer join
    await bundle.join("all_data", "bundle.id = all_data.id",
                 location="other.parquet", join_type="full")
    ```

=== "Sync API"

    ```python
    bundle.join("orders", "bundle.customer_id = orders.id",
           location="orders.parquet", join_type="left")

    bundle.join("all_data", "bundle.id = all_data.id",
           location="other.parquet", join_type="full")
    ```

=== "SQL"

    ```sql
    LEFT JOIN 'orders.parquet' AS orders ON bundle.customer_id = orders.id
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

=== "SQL"

    ```sql
    RENAME JOIN orders TO customer_orders
    ```

!!! note
    `rename_join()` is not available in the sync API yet.

!!! note
    Until you commit, join changes will not be used when the bundle is reopened.
