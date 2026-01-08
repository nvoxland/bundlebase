

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