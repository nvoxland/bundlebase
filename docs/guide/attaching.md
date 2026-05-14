# Attaching Data

Data is added to the bundle via the `.attach()` method.

## Basic Usage

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.create("my/data")
    await bundle.attach("customers.csv")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data")
    bundle.attach("customers.csv")
    ```

=== "SQL"

    ```sql
    ATTACH 'customers.csv'
    ```

## Attaching to a Join Pack

By default, data attaches to the base pack. Use the `pack` parameter to attach data to a joined pack instead.

=== "Async API"

    ```python
    await bundle.join("orders", on="customer_id = orders.id")
    await bundle.attach("orders.parquet", pack="orders")
    ```

=== "Sync API"

    ```python
    bundle.join("orders", on="customer_id = orders.id")
    bundle.attach("orders.parquet", pack="orders")
    ```

=== "SQL"

    ```sql
    ATTACH 'orders.parquet' TO orders
    ```

## Path Resolution

The `attach()` method handles paths flexibly:

- Paths can be any supported URL and the data will be read from there.
- Paths can be relative to the data_dir. But NOT `..` to a parent dir.

## Attaching From Another Bundle

You can attach the query output of another committed bundle using a `bundle://` URL. This reads the target bundle's full query output, including any filters, column operations, and joins that have been applied.

**For filesystem bundles**, use `bundle://` followed by the path:

=== "SQL"

    ```sql
    ATTACH 'bundle:///path/to/other/bundle'
    ```

=== "Async API"

    ```python
    await bundle.attach("bundle:///path/to/other/bundle")
    ```

=== "Sync API"

    ```python
    bundle.attach("bundle:///path/to/other/bundle")
    ```

**For remote bundles** (S3, etc.), use the compound scheme `bundle+<scheme>://`:

=== "SQL"

    ```sql
    ATTACH 'bundle+s3://bucket/path/to/bundle'
    ```

=== "Async API"

    ```python
    await bundle.attach("bundle+s3://bucket/path/to/bundle")
    ```

=== "Sync API"

    ```python
    bundle.attach("bundle+s3://bucket/path/to/bundle")
    ```

!!! note
    The target bundle must be committed. The attached data reflects the target's full query output at read time, including any filters, column operations, and joins that have been applied.

## Supported Formats

- CSV (`.csv`)
- TSV (`.tsv`) -- tab-separated values
- JSON Lines (`.json`, `.jsonl`)
- Parquet (`.parquet`)

!!! note
    Only JSON Lines format (one JSON object per line) can be directly attached. For arbitrary JSON files (including API responses with wrapper objects, nested structures, or JSON arrays) use a connector (`CREATE SOURCE USING http`, `remote_dir`, etc.) with `json_record_path`. The connector transforms and copies the data into the bundle as Parquet. See [Sources: JSON Options](sources.md#json-options).

## Index Refresh

Each `ATTACH` automatically refreshes every defined index against the
new block before the change lands, so queries see the new rows through
the index immediately. When bulk-loading many files in a row, suppress
that per-file rebuild and run [`REINDEX`](indexing.md#reindex) once at
the end:

```sql
ATTACH 'jan.parquet' NO INDEX;
ATTACH 'feb.parquet' NO INDEX;
ATTACH 'mar.parquet' NO INDEX;
REINDEX;
```

The same `NO INDEX` flag is available on `FETCH` and `FETCH ALL`.

## Column Types

**CSV and TSV files** are imported with all columns as text (`Utf8`). Because these are text-based formats, type inference from sampled rows is unreliable: a column that looks numeric in the first 100 rows might contain non-numeric values later. By defaulting to text, bundlebase avoids silent data corruption.

**JSON files** retain their native types (string, number, boolean) since the JSON format encodes types directly in the data.

**Parquet files** retain their native types since the schema is embedded in the file.

To convert text columns to specific types after attaching CSV data, use `cast_column`:

=== "Async API"

    ```python
    await bundle.attach("sales.csv")
    await bundle.cast_column("revenue", "float")
    await bundle.cast_column("quantity", "integer")
    ```

=== "Sync API"

    ```python
    bundle.attach("sales.csv")
    bundle.cast_column("revenue", "float")
    bundle.cast_column("quantity", "integer")
    ```

=== "SQL"

    ```sql
    ATTACH 'sales.csv'
    CAST COLUMN revenue TO float
    CAST COLUMN quantity TO integer
    ```

See [Cast Column](columns.md#cast-column) for more details.

## Detaching Data

Remove a previously attached block by its location with `detach_block()`.

=== "Async API"

    ```python
    await bundle.detach_block("customers.csv")
    ```

=== "Sync API"

    ```python
    bundle.detach_block("customers.csv")
    ```

=== "SQL"

    ```sql
    DETACH 'customers.csv'
    ```

## Replacing Data

Swap where a block's data is read from without changing the block's identity with `replace_block()`.

=== "Async API"

    ```python
    await bundle.replace_block("old_data.csv", "new_data.csv")
    ```

=== "Sync API"

    ```python
    bundle.replace_block("old_data.csv", "new_data.csv")
    ```

=== "SQL"

    ```sql
    REPLACE 'old_data.csv' WITH 'new_data.csv'
    ```
