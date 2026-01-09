# Docker for Data

> :package: Bundle your data for **easy sharing**
> 
> :headphones: Extend, **remix**, and build on top of existing bundles
> 
> :snake: All from a **Python API**

## Features

- **Multiple Formats** - Support for Parquet, CSV, and JSON data
- **Python Native** - Seamless async/sync Python API with type hints
- **High Performance** - Rust-powered core with Apache Arrow columnar format

## Quick Example

=== "Sync API"

    Create bundle:

    ```python
    import bundlebase.sync as bb

    c = bb.create("s3://mybucket/path")
        .attach("data.parquet")
        .filter("age >= 18")
        .remove_column("ssn")
        .rename_column("fname", "first_name")

    c.commit("Created initial bundle")
    ```

    Open bundle:

    ```python
    import bundlebase.sync as bb

    c = bb.open("s3://mybucket/path")
    print(c.to_pandas())
    print(bb.select("select * from bundle where revenue > 100"))
    ```

=== "Async API"

    Create bundle:

    ```python
    import bundlebase as bb

    c = await (bb.create("s3://mybucket/path")
        .attach("data.parquet")
        .filter("age >= 18")
        .remove_column("ssn")
        .rename_column("fname", "first_name"))

    await c.commit("Created initial bundle")
    ```

    Open bundle:

    ```python
    import bundlebase as bb

    c = await bb.open("s3://mybucket/path")

    print(await c.to_pandas())
    print(await bb.select("select * from bundle where revenue > 100"))
    ```

## Next Steps

1. [Installation](getting-started/install.md)
1. [Quick Start](getting-started/quick-start.md)