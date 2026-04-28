Discover and attach files from all configured sources across all packs. Mode controls behavior: ADD only adds new files, UPDATE adds new and replaces changed files, SYNC adds, updates, and removes deleted files. Use DRY RUN to preview changes without executing. Use NO INDEX to skip the automatic index refresh that normally runs after the fetch — defer the rebuild and run REINDEX explicitly when ready.

### Examples

    FETCH ALL ADD
    FETCH ALL UPDATE
    FETCH ALL SYNC
    FETCH ALL SYNC DRY RUN
    FETCH ALL SYNC NO INDEX
