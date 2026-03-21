Change a column's data type. Optionally use CLEAN with a regex pattern to strip unwanted characters before casting.

### Examples

    CAST COLUMN price TO integer
    CAST COLUMN amount TO float
    CAST COLUMN price TO integer CLEAN '[^0-9]'
