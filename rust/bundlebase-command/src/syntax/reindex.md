Rebuild all indexes in the bundle against the current set of blocks. Indexes
are normally refreshed automatically after every ATTACH, REPLACE, and FETCH;
you only need to run REINDEX explicitly after using `NO INDEX` to suppress
that auto-refresh.

### Examples

    REINDEX
