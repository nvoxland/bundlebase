---
date: 2026-02-19
categories:
  - Releases
---

# Bundlebase 0.6.0

Two big additions: text search with the new `search()` table function, and the ability to attach and join other bundles.

<!-- more -->

## New Features

### Text search

The way text indexes work was completely revamped. You now create text indexes and query them with the `search()` table function:

```python
bundle.create_index(["Company"], "text", name="company_search")
bundle.commit("Created text index")

result = bundle.query(
    "SELECT Company, _score FROM search('company_search', 'Group') ORDER BY _score DESC"
)
```

Multi-column indexes and field-specific queries work too:

```python
bundle.create_index(["Company", "City"], "text", name="company_city")
bundle.commit("Created multi-column index")

result = bundle.query(
    "SELECT * FROM search('company_city', 'Company:group') ORDER BY _score DESC LIMIT 10"
)
```

If there's only one text index on the bundle, you can skip the name:

```sql
SELECT * FROM search('machine learning')
```

Results include a `_score` column with BM25 relevance scores. Multiple tokenizers are available including stemming for English, German, French, Spanish, and others — pass `args={"tokenizer": "en_stem"}` when creating the index.

See the [text search guide](../../guide/text-search.md) for the full details.

### Attaching and joining bundles

Bundles can now reference other bundles. Attach pulls in another bundle's data, and join links them on a key:

```python
# Attach another bundle's data
bundle.attach("bundle:///path/to/other/bundle")

# Join bundles on a key
bundle.join("regions", 'base."Country" = regions."Country"',
       "bundle:///path/to/regions/bundle")
```

Remote bundles work too — use `bundle+s3://bucket/path` for S3-backed bundles.

The attached/joined data reflects the target bundle's current state, including any filters or transformations applied to it.

See the [attaching guide](../../guide/attaching.md) and [joins guide](../../guide/joins.md).

### CLI renamed to `bundlebase`

The CLI binary is now just `bundlebase` instead of `bundlebase-cli`.

## Other changes

- **Index caching** — deserialized indexes are now cached in an LRU cache, avoiding repeated disk reads for hot queries
- **Better `explain()` and `query()` output** — improved formatting in the Python API

## Breaking Changes

### `text_search()` replaced by `search()`

The old `text_search()` UDF is gone. Use the `search()` table function instead, which supports named indexes, multi-column queries, and relevance scoring.

### CLI binary name

If you have scripts referencing `bundlebase-cli`, update them to `bundlebase`.

---

```
pip install bundlebase==0.6.0
```

Let me know if you run into issues, especially with the new search function or bundle-to-bundle joins.
