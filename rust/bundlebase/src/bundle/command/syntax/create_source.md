Create a data source for automatic file discovery. Sources define where to find data files using a connector and its arguments. Use ON to target a specific join pack.

### Examples

    CREATE SOURCE remote_dir WITH (url = 's3://bucket/data/', patterns = '**/*.parquet')
    CREATE SOURCE remote_dir WITH (url = 'file:///data/') ON users
    CREATE SOURCE acme.weather WITH (api_key = 'abc123')
    CREATE SOURCE acme.weather
