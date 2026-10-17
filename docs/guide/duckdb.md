---
seo_title: "Query Bundlebase with DuckDB over Arrow"
---

# DuckDB

Use `arrow_reader()` to query bundle data in DuckDB through a standard
`pyarrow.RecordBatchReader`. Install DuckDB separately (`pip install duckdb`);
Bundlebase does not require it at runtime. Interoperability coverage uses DuckDB
1.5.6 and PyArrow 23.0.1; these are tested versions, not minimum version guarantees.

## Query a bundle

```python
import bundlebase.sync as bb
import duckdb

bundle = bb.open("sales.bundle")
reader = bundle.query(
    "SELECT region, amount FROM bundle WHERE year = $1", [2026]
).arrow_reader()

with duckdb.connect() as con:
    con.register("recent_sales", reader)
    totals = con.sql(
        "SELECT region, SUM(amount) AS total FROM recent_sales GROUP BY region"
    ).fetchall()
reader.close()
```

`arrow_reader()` is available on bundles, builders, and query results. A bundle
or builder call creates a fresh stream. Calling it on a query result transfers
that result's stream to the reader; subsequent conversions of the result fail.
The reader preserves the prepared query's schema, including typed empty results.

Select columns and filter rows in the Bundlebase query before export when you
want Bundlebase's normal source pruning. DuckDB receives the issued stream;
DuckDB filters do not push predicates back into Bundlebase or reduce source IO.
Bundlebase SQL uses DataFusion syntax, while SQL executed on `con` uses DuckDB
syntax.

## Async applications

Creating the reader is async in the async API. Reading it and executing DuckDB
queries are synchronous. Move blocking work to a worker thread when the event
loop needs to keep running:

```python
import asyncio
import bundlebase as bb
import duckdb


def count_rows(reader):
    try:
        with duckdb.connect() as con:
            con.register("sales", reader)
            return con.sql("SELECT COUNT(*) FROM sales").fetchone()[0]
    finally:
        reader.close()


async def main():
    bundle = await bb.open("sales.bundle")
    reader = await bundle.arrow_reader()
    return await asyncio.to_thread(count_rows, reader)
```

For native async batch processing, use `stream_batches()`.

## Read again

A reader is sequential and one-shot. At the end, Arrow iteration stops normally.
Registering it with DuckDB does not make it replayable. Create and register a
fresh reader for another scan:

```python
con.register("sales", bundle.arrow_reader())
```

For repeated queries, explicitly materialize the data inside DuckDB:

```python
with duckdb.connect() as con:
    reader = bundle.arrow_reader()
    try:
        con.register("incoming", reader)
        con.execute("CREATE TABLE sales AS SELECT * FROM incoming")
        con.sql("SELECT COUNT(*) FROM sales").show()
        con.sql("SELECT region, SUM(amount) FROM sales GROUP BY region").show()
    finally:
        reader.close()
```

The bridge reads batches on demand without accumulating the full result. The
query engines and consumers can still allocate memory according to their plans.
Close a partially consumed reader to release its query resources. Batches you
already read remain valid after closing the reader.

## Save DuckDB results in a bundle

Export computed results to Parquet, then attach the file and commit through the
ordinary Bundlebase API:

```python
with duckdb.connect() as con:
    reader = bundle.arrow_reader()
    try:
        con.register("incoming", reader)
        con.execute("""
            COPY (
                SELECT region, SUM(amount) AS total
                FROM incoming GROUP BY region
            ) TO 'regional_totals.parquet' (FORMAT PARQUET)
        """)
    finally:
        reader.close()

output = bb.create("regional-totals.bundle")
output.attach("regional_totals.parquet")
output.commit("Regional totals computed with DuckDB")
```

The reader API exports query results. Bundle changes continue through Bundlebase
operations and commits.
