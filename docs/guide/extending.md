# Extending

Bundles can extend other bundles to build on previous versions.

## From Chain

=== "Async API"

    ```python
    import bundlebase as bb

    # Load committed bundle
    base = await bb.open("/base/container")

    # Extend to new directory with new modifications
    extended = await base.extend("/extended/container")
    await extended.attach("new_data.parquet")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    base = bb.open("/base/container")

    extended = base.extend("/extended/container")
    extended.attach("new_data.parquet")
    ```

## Exporting

Export the entire bundle as a tar archive.

=== "Async API"

    ```python
    await c.export_tar("/path/to/output.tar")
    ```

=== "Sync API"

    ```python
    c.export_tar("/path/to/output.tar")
    ```
