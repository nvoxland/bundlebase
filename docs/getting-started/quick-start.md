# Quick Start

This guide will get you started with Bundlebase in just a few minutes. We'll cover both the async and sync APIs.

## Installation

To install Bundlebase, see [the installation guide](install.md)

## Choose Your API Style

Bundlebase offers two API styles for maximum flexibility:

- Async/await interface - ideal for concurrent operations and production code.
- Synchronous interface - perfect for scripts and Jupyter notebooks.

Make sure you are importing the version you want to use

=== "Async API"

    ```python
    import bundlebase as bb

    # All operations use await
    c = await bb.create()
    c = await c.attach("data.parquet")
    df = await c.to_pandas()
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    # No await needed!
    c = bb.create()
    c = bb.attach("data.parquet")
    df = bb.to_pandas()
    ```

## Creating Your First Bundle

You create your bundle in a given directory where all data will be saved. 

The data_dir can be a local filepath or a remote URL (S3, Azure, GCS)

=== "Async API"

    ```python
    import bundlebase as bb

    # Create a new bundle
    c = await bb.create("s3://mybucket/path")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    # Create a new bundle
    c = bb.create("s3://mybucket/path")
    ```

## Attaching Data

A bundle is no use without data, add it with `attach()`.

- Bundlebase supports multiple data formats: Parquet, CSV, and JSON.
- Attaching multiple files unions the data together -- even across data types
- Datafile paths can be relative to the data_dir OR an absolute URL to anywhere

=== "Async API"

    ```python
    # Attach data sources
    await c.attach("local_data.parquet")
    await c.attach("s3://other_bucket/more_data.csv")
    await c.attach("https://example.com/additional.json")
    ```

=== "Sync API"

    ```python
    # Attach data sources
    c.attach("local_data.parquet")
    c.attach("s3://other_bucket/more_data.csv")
    c.attach("https://example.com/additional.json")
    ```

## Transforming Data

Bundlebase provides APIs for transforming your data:

=== "Async API"

    ```python
    # Filter rows
    await c.filter("age >= 18")

    # Remove columns
    await c.remove_column("ssn")

    # Rename columns
    await c.rename_column("fname", "first_name")
    ```

=== "Sync API"

    ```python
    # Filter rows
    c.filter("age >= 18")

    # Remove columns
    c.remove_column("ssn")

    # Rename columns
    c.rename_column("fname", "first_name")
    ```

## Committing Changes

When you are happy with your bundle, commit the state to disk so it can be re-opened and shared:


=== "Async API"

    ```python
    await c.commit("Initial commit")

    ## then later...
    c = bb.open("s3://mybucket/path")
    ```

=== "Sync API"

    ```python
    c.commit("Initial commit")

    ## then later...
    c = bb.open("s3://mybucket/path")
    ```

## Method Chaining

All mutation methods return `self`, enabling clean method chaining:

=== "Async API"

    ```python
    c = await (bb.create()
        .attach("data.parquet")
        .filter("active = true")
        .remove_column("temp_field")
        .rename_column("old_name", "new_name"))
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    c = (bb.create()
        .attach("data.parquet")
        .filter("active = true")
        .remove_column("temp_field")
        .rename_column("old_name", "new_name"))
    ```

## Exporting Results

Convert your bundle to your preferred data format:

=== "Async API"

    ```python
    # To pandas DataFrame
    df = await c.to_pandas()

    # To Polars DataFrame
    df = await c.to_polars()

    # To NumPy arrays
    arrays = await c.to_numpy()

    # To Python dictionary
    data = await c.to_dict()
    ```

=== "Sync API"

    ```python
    # To pandas DataFrame
    df = c.to_pandas()

    # To Polars DataFrame
    df = c.to_polars()

    # To NumPy arrays
    arrays = c.to_numpy()

    # To Python dictionary
    data = c.to_dict()
    ```

## Querying the Bundle

You can run standard SQL queries against your bundle. 

Bundlebase supports [Apache Datafusion SQL syntax](https://datafusion.apache.org/user-guide/sql/index.html)


=== "Async API"

    ```python
    rs = await c.select("select * from bundle where revenue > 100")
    
    # Can export the rs like a bundle
    df = rs.to_polars()
    ```

=== "Sync API"

    ```python
    rs = c.select("select * from bundle where revenue > 100")
    
    # Can export the rs like a bundle
    df = rs.to_polars()
    ```

## Next Steps

- **[Basic Concepts](basic-concepts.md)** - Learn about bundles, operations, and versioning
- **[User Guide](../guide/attaching.md)** - Deep dive into advanced topics
- **[API Reference](../api/python/index.md)** - Complete API documentation
- **[Examples](../examples/basic-operations.md)** - More examples