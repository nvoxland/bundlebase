Attach a data file to the bundle. Supported formats include CSV, Parquet, and JSON.
Use TO to attach to a specific join pack. Use WITH for format-specific options.
Use NO INDEX to skip the automatic index refresh that normally runs after the
attach — handy when bulk-loading; run REINDEX once at the end.

### Examples

    ATTACH 'data.csv'
    ATTACH 'data.parquet' TO users
    ATTACH 'data.csv' WITH (delimiter = '|', header = true)
    ATTACH 'jan.parquet' NO INDEX
