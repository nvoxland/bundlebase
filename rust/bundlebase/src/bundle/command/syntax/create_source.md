Create a data source for automatic file discovery. Sources define where to find data files using a connector and its arguments. Use FOR to target a specific join pack (defaults to base).

### Examples

    CREATE SOURCE USING remote_dir WITH (url = 's3://bucket/data/', patterns = '**/*.parquet')
    CREATE SOURCE FOR users USING remote_dir WITH (url = 'file:///data/')
    CREATE SOURCE USING acme.weather WITH (api_key = 'abc123')
    CREATE SOURCE USING acme.weather
