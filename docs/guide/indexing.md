# Indexing

While Bundlebase can query any attached data, the base formats are not always the most efficient to query.

Creating indexes on columns you are frequently filtering on will allow for faster query execution.

=== "Async API"

    ```python
    # Create index on email column
    await c.create_index("email")
    c.commit("Added email index")
    ```

=== "Sync API"

    ```python
    # Create index on email column
    c.create_index("email")
    c.commit("Added email index")
    ```
!!! note

    Until you commit, the index will not be used when the bundle is reopened. 