Delete rows matching a WHERE condition. Deleted rows are excluded from all subsequent queries.

Deletes accumulate in memory until COMMIT, when a single tombstone file is written.

### Examples

    DELETE FROM bundle WHERE salary < 0
    DELETE FROM bundle WHERE status = 'inactive' AND last_login < '2020-01-01'
    DELETE FROM bundle WHERE id IN (1, 2, 3)
