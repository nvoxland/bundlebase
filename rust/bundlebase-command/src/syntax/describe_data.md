Returns per-column statistics: min, max, average, null counts, most frequent values, and values that fail to cast to an expected type.

Use `AS <type>` to detect sentinel values — values that don't match the expected SQL type (e.g., `BIGINT`, `DOUBLE`, `VARCHAR`).

### Examples

    DESCRIBE DATA IN salary, first_name
    DESCRIBE DATA IN secchi_depth_m AS DOUBLE, station_name
    DESCRIBE DATA IN id AS BIGINT, "Full Name"
