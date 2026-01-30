# Attaching Data

Data is added to the bundle via the `.attach()` method.

## Basic Usage

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.create("my/data")
    await bundle.attach("customers.csv")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data")
    bundle.attach("customers.csv")
    ```

=== "SQL"

    ```sql
    ATTACH 'customers.csv'
    ```

## Attaching to a Join Pack

By default, data attaches to the base pack. Use the `pack` parameter to attach data to a joined pack instead.

=== "Async API"

    ```python
    await bundle.join("orders", on="customer_id = orders.id")
    await bundle.attach("orders.parquet", pack="orders")
    ```

=== "Sync API"

    ```python
    bundle.join("orders", on="customer_id = orders.id")
    bundle.attach("orders.parquet", pack="orders")
    ```

=== "SQL"

    ```sql
    ATTACH 'orders.parquet' TO orders
    ```

## Path Resolution

The `attach()` method handles paths flexibly:

- Paths can be any supported URL and the data will be read from there.
- Paths can be relative to the data_dir. But NOT `..` to a parent dir.

## Supported Formats

- CSV
- JSON Line
- Parquet

## Detaching Data

Remove a previously attached block by its location with `detach_block()`.

=== "Async API"

    ```python
    await bundle.detach_block("customers.csv")
    ```

=== "Sync API"

    ```python
    bundle.detach_block("customers.csv")
    ```

=== "SQL"

    ```sql
    DETACH 'customers.csv'
    ```

## Replacing Data

Swap where a block's data is read from without changing the block's identity with `replace_block()`.

=== "Async API"

    ```python
    await bundle.replace_block("old_data.csv", "new_data.csv")
    ```

=== "Sync API"

    ```python
    bundle.replace_block("old_data.csv", "new_data.csv")
    ```

=== "SQL"

    ```sql
    REPLACE 'old_data.csv' WITH 'new_data.csv'
    ```
