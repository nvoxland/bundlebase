# Extending

Bundles can extend other bundles to build on previous versions.

## Extend a Bundle

`extend()` turns an opened (read-only) bundle into a builder you can modify. Pass a data directory to write new data files somewhere other than the base bundle's directory.

=== "Async API"

    ```python
    import bundlebase as bb

    # Load committed bundle
    base = await bb.open("/base/bundle")

    # Extend with a new data directory, then modify
    extended = await base.extend("/extended/data")
    await extended.attach("new_data.parquet")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    base = bb.open("/base/bundle")

    extended = base.extend("/extended/data")
    extended.attach("new_data.parquet")
    ```

## Exporting

Export the entire bundle as a tar archive. Pass `gzip=True` to write a `.tar.gz` instead.

=== "Async API"

    ```python
    await extended.export_tar("/path/to/output.tar")
    ```

=== "Sync API"

    ```python
    extended.export_tar("/path/to/output.tar")
    ```
