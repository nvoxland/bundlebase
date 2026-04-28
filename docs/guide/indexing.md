# Indexing

While Bundlebase can query any attached data, the base formats are not always the most efficient to query.

Creating indexes on columns you frequently filter on will allow for faster query execution.

## BTree Indexes

BTree indexes accelerate exact-match lookups and range queries. Use `create_index()` with `index_type="btree"`.

=== "Async API"

    ```python
    await bundle.create_index("email", index_type="btree")
    ```

=== "Sync API"

    ```python
    bundle.create_index("email", index_type="btree")
    ```

=== "SQL"

    ```sql
    CREATE BTREE INDEX ON email
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

## Automatic Reindexing

Indexes stay in sync automatically. After every `ATTACH`, `REPLACE`, or
`FETCH`, Bundlebase folds an index-rebuild for the new/replaced blocks
into the same change, so queries see freshly-attached data through the
index immediately — no manual `REINDEX` step needed.

To opt out — for example, when bulk-loading many files and you'd rather
rebuild once at the end — pass `NO INDEX`:

```sql
ATTACH 'jan.parquet' NO INDEX;
ATTACH 'feb.parquet' NO INDEX;
FETCH base ADD NO INDEX;
REINDEX;   -- run once when you're ready
```

Old index files are kept on disk and stay referenced by older commits, so
opening the bundle pinned to a previous version still finds the matching
index data.

## Reindex

Create index files for any blocks that are missing them. This checks
existing indexes and avoids redundant work — you typically only need to
call it explicitly after using `NO INDEX`.

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
