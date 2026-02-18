# Text Search

Bundlebase includes full-text search powered by [Tantivy](https://github.com/quickwit-oss/tantivy) with [BM25](https://en.wikipedia.org/wiki/Okapi_BM25) relevance ranking. Text indexes can span one or more columns, and search results include a `_score` column for ranking.

## Creating a Text Index

Use `create_index()` with one or more columns and `index_type="text"`. You can optionally provide a `name` for the index (used when querying). If no name is provided, the first text index defaults to `"full_text"`. Additional text indexes are auto-named as `idx_{columns joined by _}`.

=== "Async API"

    ```python
    # Single-column text index (auto-named "full_text" — the first text index)
    await bundle.create_index("description", "text")

    # Additional text index (auto-named "idx_title_description")
    await bundle.create_index(["title", "description"], "text")

    # With an explicit name
    await bundle.create_index(["title", "description"], "text", name="product_search")

    # With a custom tokenizer
    await bundle.create_index("content", "text", args={"tokenizer": "en_stem"})

    await bundle.commit("Created text indexes")
    ```

=== "Sync API"

    ```python
    bundle.create_index("description", "text")
    bundle.create_index(["title", "description"], "text")
    bundle.create_index(["title", "description"], "text", name="product_search")
    bundle.create_index("content", "text", args={"tokenizer": "en_stem"})

    bundle.commit("Created text indexes")
    ```

!!! note
    Until you commit, the index will not be used when the bundle is reopened.

### Multi-Column Indexes

A multi-column index stores all specified columns in a single search index. This lets you search across columns in a single query using field-specific syntax, and get a unified relevance score.

```python
await bundle.create_index(["title", "description", "tags"], "text", name="product_search")
```

## Querying

Use the `search()` table function to perform full-text search. It takes the index name and a query string, and replaces `FROM bundle` in your SQL.

Results include all columns from the bundle plus a `_score` column with the BM25 relevance score.

=== "SQL"

    ```sql
    -- Basic search
    SELECT * FROM search('desc_search', 'machine learning')

    -- Order by relevance
    SELECT title, description, _score
    FROM search('product_search', 'machine learning')
    ORDER BY _score DESC
    LIMIT 10

    -- Additional WHERE filters on top of search results
    SELECT * FROM search('product_search', 'machine learning')
    WHERE category = 'AI'
    ```

=== "Async API"

    ```python
    results = await bundle.query(
        "SELECT title, _score FROM search('product_search', 'machine learning') ORDER BY _score DESC"
    )
    df = await results.to_polars()
    ```

=== "Sync API"

    ```python
    results = bundle.query(
        "SELECT title, _score FROM search('product_search', 'machine learning') ORDER BY _score DESC"
    )
    df = results.to_polars()
    ```

### Query Syntax

The query string uses [Tantivy query syntax](https://docs.rs/tantivy/latest/tantivy/query/struct.QueryParser.html):

| Syntax | Example | Description |
|--------|---------|-------------|
| Terms | `machine learning` | Matches rows containing any of the words (OR by default) |
| Required terms | `+machine +learning` | Matches rows containing both words |
| Phrases | `"machine learning"` | Matches the exact phrase |
| Field-specific | `title:learning` | Searches only the specified column |
| Combined fields | `+title:machine +description:learning` | Different terms in different columns |

### Field-Specific Queries

With multi-column indexes, you can target specific columns using the `column:term` syntax. The column names match the columns specified when creating the index.

```sql
-- Search only the title column
SELECT * FROM search('product_search', 'title:learning')

-- Require matches in both title and description
SELECT * FROM search('product_search', '+title:machine +description:learning')
```

When no field is specified, all columns in the index are searched.

### The Score Column

Every `search()` query returns a `_score` column (Float64) with the BM25 relevance score. Higher scores indicate better matches. Use `ORDER BY _score DESC` to rank results by relevance.

```sql
SELECT title, _score
FROM search('product_search', 'machine learning')
ORDER BY _score DESC
LIMIT 10
```

## Tokenizers

When creating a text index, you can specify a tokenizer to control how text is split and normalized. The default is `simple`.

| Tokenizer | Aliases | Description |
|-----------|---------|-------------|
| `simple` | | Splits on whitespace and lowercases. Good general-purpose default. |
| `raw` | | No tokenization — treats the entire field value as a single token. Use for exact-match fields like IDs or codes. |
| `en_stem` | `english_stem`, `english` | English stemming with stop word removal. Matches word variants (e.g., "running" matches "run"). |
| `de_stem` | `german_stem`, `german` | German stemming with stop word removal. |
| `fr_stem` | `french_stem`, `french` | French stemming with stop word removal. |
| `es_stem` | `spanish_stem`, `spanish` | Spanish stemming with stop word removal. |
| `it_stem` | `italian_stem`, `italian` | Italian stemming with stop word removal. |
| `pt_stem` | `portuguese_stem`, `portuguese` | Portuguese stemming with stop word removal. |
| `nl_stem` | `dutch_stem`, `dutch` | Dutch stemming with stop word removal. |
| `sv_stem` | `swedish_stem`, `swedish` | Swedish stemming with stop word removal. |
| `no_stem` | `norwegian_stem`, `norwegian` | Norwegian stemming with stop word removal. |
| `da_stem` | `danish_stem`, `danish` | Danish stemming with stop word removal. |
| `fi_stem` | `finnish_stem`, `finnish` | Finnish stemming with stop word removal. |
| `ru_stem` | `russian_stem`, `russian` | Russian stemming with stop word removal. |
