# Indexing

While Bundlebase can query any attached data, the base formats are not always the most efficient to query.

Creating indexes on columns you frequently filter on will allow for faster query execution.

## Creating Indexes

The `create_index()` method requires a column name and an index type (`"column"` or `"text"`).

=== "Async API"

    ```python
    # Column index for exact lookups and range queries
    await bundle.create_index("email", index_type="column")

    # Text index for full-text search
    await bundle.create_index("description", index_type="text")

    # Text index with a custom tokenizer
    await bundle.create_index("content", index_type="text", options={"tokenizer": "en_stem"})
    ```

=== "Sync API"

    ```python
    bundle.create_index("email", index_type="column")
    bundle.create_index("description", index_type="text")
    bundle.create_index("content", index_type="text", options={"tokenizer": "en_stem"})
    ```

=== "SQL"

    ```sql
    CREATE INDEX ON email
    ```

!!! note
    Until you commit, the index will not be used when the bundle is reopened.

## Drop Index

Remove an index from a column.

=== "Async API"

    ```python
    await bundle.drop_index("email")
    ```

=== "Sync API"

    ```python
    bundle.drop_index("email")
    ```

=== "SQL"

    ```sql
    DROP INDEX email
    ```

## Rebuild Index

Rebuild an existing index on a column. Useful if the index has become stale or corrupted.

=== "Async API"

    ```python
    await bundle.rebuild_index("email")
    ```

=== "Sync API"

    ```python
    bundle.rebuild_index("email")
    ```

=== "SQL"

    ```sql
    REBUILD INDEX ON email
    ```

## Reindex

Create index files for any blocks that are missing them. This checks existing indexes and avoids redundant work.

=== "Async API"

    ```python
    await bundle.reindex()
    ```

=== "Sync API"

    ```python
    bundle.reindex()
    ```

=== "SQL"

    ```sql
    REINDEX
    ```
