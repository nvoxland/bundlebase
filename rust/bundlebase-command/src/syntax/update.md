Update rows matching a WHERE condition with new values. Updated values are stored in overlay parquet files and merged at query time. Original data files are not modified.

SET expressions can reference other columns and use SQL functions.

### Examples

    UPDATE bundle SET salary = salary * 1.1 WHERE department = 'eng'
    UPDATE bundle SET status = NULL WHERE inactive = true
    UPDATE bundle SET name = 'unknown', age = 0 WHERE name IS NULL
