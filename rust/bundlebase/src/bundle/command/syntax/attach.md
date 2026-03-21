Attach a data file to the bundle. Supported formats include CSV, Parquet, and JSON.
Use TO to attach to a specific join pack. Use WITH for format-specific options.

### Examples

    ATTACH 'data.csv'
    ATTACH 'data.parquet' TO users
    ATTACH 'data.csv' WITH (delimiter = '|', header = true)
