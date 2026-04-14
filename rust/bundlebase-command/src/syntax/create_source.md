Create a data source for automatic file discovery. Sources define where to find data files using a connector and its arguments. Use FOR to target a specific join pack (defaults to base).

The optional `SAVE AS` clause controls how fetched data is stored: `AUTO` (default) always converts to Parquet and stores in the bundle; `COPY` downloads into the bundle; `PARQUET` converts to Parquet; `REF` references the URL directly.

The optional `MIN BATCH` clause sets the minimum combined batch size for merging small fetched files. Use a human-readable size such as `15M` or `3G`.

### Syntax

    CREATE SOURCE [FOR <pack>] USING <connector> [WITH (<args>)] [SAVE AS <AUTO|COPY|PARQUET|REF>] [MIN BATCH <size>]

### Examples

    CREATE SOURCE USING remote_dir WITH (url = 's3://bucket/data/', patterns = '**/*.parquet')
    CREATE SOURCE FOR users USING remote_dir WITH (url = 'file:///data/')
    CREATE SOURCE USING http WITH (url = 'https://example.com/data.csv') SAVE AS COPY
    CREATE SOURCE USING remote_dir WITH (url = 's3://bucket/data/', patterns = '**/*.jsonl') MIN BATCH 15M
    CREATE SOURCE USING http WITH (url = 'https://example.com/data.xlsx') SAVE AS PARQUET
    CREATE SOURCE USING acme.weather WITH (api_key = 'abc123')
