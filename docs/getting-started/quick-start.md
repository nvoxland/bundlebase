# Quick Start

This guide will get you started with Bundlebase in just a few minutes. We'll cover both the async and sync APIs.

## Choose Your API Style

Bundlebase offers two API styles for maximum flexibility:

=== "Async API"

    Modern async/await interface - ideal for concurrent operations and production code.

    ```python
    import bundlebase

    # All operations use await
    c = await bundlebase.create()
    c = await c.attach("data.parquet")
    df = await c.to_pandas()
    ```

=== "Sync API"

    Synchronous interface - perfect for scripts and Jupyter notebooks.

    ```python
    import bundlebase.sync as dc

    # No await needed!
    c = dc.create()
    c = c.attach("data.parquet")
    df = c.to_pandas()
    ```

Choose the tab above that matches your preferred style - all examples below will update accordingly.

## Creating Your First Bundle

=== "Async API"

    ```python
    import bundlebase

    # Create a new bundle
    c = await bundlebase.create()
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as dc

    # Create a new bundle
    c = dc.create()
    ```

## Loading Data

Bundlebase supports multiple data formats: Parquet, CSV, JSON, and more.

=== "Async API"

    ```python
    # Attach data sources
    c = await c.attach("data.parquet")
    c = await c.attach("more_data.csv")
    c = await c.attach("additional.json")
    ```

=== "Sync API"

    ```python
    # Attach data sources
    c = c.attach("data.parquet")
    c = c.attach("more_data.csv")
    c = c.attach("additional.json")
    ```

## Transforming Data

Bundlebase provides a fluent API for transforming your data:

=== "Async API"

    ```python
    # Filter rows
    c = await c.filter("age >= 18")

    # Select specific columns
    c = await c.select(["id", "name", "email"])

    # Remove columns
    c = await c.remove_column("ssn")

    # Rename columns
    c = await c.rename_column("fname", "first_name")
    ```

=== "Sync API"

    ```python
    # Filter rows
    c = c.filter("age >= 18")

    # Select specific columns
    c = c.select(["id", "name", "email"])

    # Remove columns
    c = c.remove_column("ssn")

    # Rename columns
    c = c.rename_column("fname", "first_name")
    ```

## Method Chaining

All mutation methods return `self`, enabling clean method chaining:

=== "Async API"

    ```python
    import bundlebase

    c = await (bundlebase.create()
        .attach("data.parquet")
        .filter("active = true")
        .remove_column("temp_field")
        .rename_column("old_name", "new_name")
        .select(["id", "name", "value"]))
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as dc

    c = (dc.create()
        .attach("data.parquet")
        .filter("active = true")
        .remove_column("temp_field")
        .rename_column("old_name", "new_name")
        .select(["id", "name", "value"]))
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

## Complete Example: Data Cleaning Pipeline

Here's a complete example showing a typical data cleaning workflow:

=== "Async API"

    ```python
    import bundlebase

    # Create and chain operations
    c = await (bundlebase.create()
        .attach("raw_customers.csv")
        .filter("status = 'active'")
        .remove_column("email")      # Remove PII
        .remove_column("phone")      # Remove PII
        .rename_column("fname", "first_name")
        .rename_column("lname", "last_name")
        .select(["id", "first_name", "last_name", "country"]))

    # Convert to pandas and save
    df = await c.to_pandas()
    print(f"Processed {len(df)} active customers")

    # Commit changes for versioning
    await c.commit("Cleaned customer data, removed PII")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as dc

    # Create and chain operations
    c = (dc.create()
        .attach("raw_customers.csv")
        .filter("status = 'active'")
        .remove_column("email")      # Remove PII
        .remove_column("phone")      # Remove PII
        .rename_column("fname", "first_name")
        .rename_column("lname", "last_name")
        .select(["id", "first_name", "last_name", "country"]))

    # Convert to pandas and save
    df = c.to_pandas()
    print(f"Processed {len(df)} active customers")

    # Commit changes for versioning
    c.commit("Cleaned customer data, removed PII")
    ```

## Joining Data

Combine multiple datasets with joins:

=== "Async API"

    ```python
    import bundlebase

    # Start with customers
    c = await (bundlebase.create()
        .attach("customers.parquet"))

    # Join with orders
    c = await c.join(
        "orders.parquet",
        left_on="customer_id",
        right_on="id",
        join_type="inner"
    )

    # Export joined data
    df = await c.to_pandas()
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as dc

    # Start with customers
    c = (dc.create()
        .attach("customers.parquet"))

    # Join with orders
    c = c.join(
        "orders.parquet",
        left_on="customer_id",
        right_on="id",
        join_type="inner"
    )

    # Export joined data
    df = c.to_pandas()
    ```

## Working with Large Datasets

For datasets larger than RAM, use streaming:

=== "Async API"

    ```python
    import bundlebase

    # Open large dataset
    c = await bundlebase.open("huge_dataset.parquet")

    # Stream batches instead of loading everything
    total_rows = 0
    async for batch in bundlebase.stream_batches(c):
        # Each batch is ~100MB, not entire dataset
        total_rows += batch.num_rows
        # Memory is freed after each iteration

    print(f"Processed {total_rows} rows")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as dc

    # Open large dataset
    c = dc.create().attach("huge_dataset.parquet")

    # Stream batches instead of loading everything
    total_rows = 0
    for batch in dc.stream_batches(c):
        # Each batch is ~100MB, not entire dataset
        total_rows += batch.num_rows
        # Memory is freed after each iteration

    print(f"Processed {total_rows} rows")
    ```

## Version Control with Commits

Bundlebase includes built-in versioning similar to Git:

=== "Async API"

    ```python
    import bundlebase

    # Create and process data
    c = await (bundlebase.create("/path/to/bundle")
        .attach("data.parquet")
        .filter("year >= 2020"))

    # Commit changes
    await c.commit("Filtered to 2020 and later")

    # Later, load the saved bundle
    c = await bundlebase.open("/path/to/bundle")
    df = await c.to_pandas()
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as dc

    # Create and process data
    c = (dc.create("/path/to/bundle")
        .attach("data.parquet")
        .filter("year >= 2020"))

    # Commit changes
    c.commit("Filtered to 2020 and later")

    # Later, load the saved bundle
    c = dc.open("/path/to/bundle")
    df = c.to_pandas()
    ```

## Using in Jupyter Notebooks

For Jupyter notebooks, use the sync API with the jupyter extra:

```bash
pip install "bundlebase[jupyter]"
```

Then in your notebook:

```python
import bundlebase.sync as dc

c = (dc.create()
    .attach("data.parquet")
    .filter("active = true"))

# Display results
display(c.to_pandas())
```

## Next Steps

Now that you understand the basics:

- **[Basic Concepts](basic-concepts.md)** - Learn about bundles, operations, and versioning
- **[API Reference](../api-reference/python/index.md)** - Complete API documentation
- **[Guides](../guides/architecture.md)** - Deep dive into advanced topics
- **[Examples](../examples/basic-operations.md)** - More practical examples

## Common Patterns

### Parameterized Filters

Use parameters to make your queries safe and reusable:

=== "Async API"

    ```python
    # Using parameterized queries (prevents SQL injection)
    min_age = 18
    c = await c.filter("age >= $1", [min_age])
    ```

=== "Sync API"

    ```python
    # Using parameterized queries (prevents SQL injection)
    min_age = 18
    c = c.filter("age >= $1", [min_age])
    ```

### SQL Queries

For complex transformations, use SQL:

=== "Async API"

    ```python
    # Execute SQL query
    c = await c.select("""
        SELECT
            id,
            name,
            CASE WHEN age >= 18 THEN 'adult' ELSE 'minor' END as age_group
        FROM self
        WHERE active = true
        LIMIT 100
    """)
    ```

=== "Sync API"

    ```python
    # Execute SQL query
    c = c.select("""
        SELECT
            id,
            name,
            CASE WHEN age >= 18 THEN 'adult' ELSE 'minor' END as age_group
        FROM self
        WHERE active = true
        LIMIT 100
    """)
    ```

### Indexing for Performance

Create indexes for faster lookups:

=== "Async API"

    ```python
    # Create index on email column
    c = await c.create_index("email")

    # Rebuild if needed
    c = await c.rebuild_index("email")
    ```

=== "Sync API"

    ```python
    # Create index on email column
    c = c.create_index("email")

    # Rebuild if needed
    c = c.rebuild_index("email")
    ```
