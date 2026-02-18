# Indexing

While Bundlebase can query any attached data, the base formats are not always the most efficient to query.

Creating indexes on columns you frequently filter on will allow for faster query execution.

## Column Indexes

Column indexes accelerate exact-match lookups and range queries. Use `create_index()` with `index_type="column"`.

=== "Async API"

    ```python
    await bundle.create_index("email", index_type="column")
    ```

=== "Sync API"

    ```python
    bundle.create_index("email", index_type="column")
    ```

=== "SQL"

    ```sql
    CREATE COLUMN INDEX ON email
    ```

!!! note
    Until you commit, the index will not be used when the bundle is reopened.

## Text Indexes

Text indexes enable full-text search with BM25 ranking. Use `create_index()` with `index_type="text"` and one or more columns. If no name is provided, indexes are auto-named as `idx_{columns}`.

=== "Async API"

    ```python
    # Single-column text index (auto-named "idx_description")
    await bundle.create_index("description", "text")

    # Multi-column text index (auto-named "idx_title_description")
    await bundle.create_index(["title", "description"], "text")

    # With explicit name and tokenizer
    await bundle.create_index("content", "text", name="content_search", args={"tokenizer": "en_stem"})
    ```

=== "Sync API"

    ```python
    bundle.create_index("description", "text")
    bundle.create_index(["title", "description"], "text")
    bundle.create_index("content", "text", name="content_search", args={"tokenizer": "en_stem"})
    ```

For more details on querying and tokenizers, see [Text Search](text-search.md).

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
