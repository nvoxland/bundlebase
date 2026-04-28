Discover and attach files from a specific pack's sources. Mode controls behavior: ADD only adds new files, UPDATE adds new and replaces changed files, SYNC adds, updates, and removes deleted files. Use DRY RUN to preview changes without executing. Use NO INDEX to skip the automatic index refresh that normally runs after the fetch — defer the rebuild and run REINDEX explicitly when ready.

### Examples

    FETCH base ADD
    FETCH users UPDATE
    FETCH orders SYNC
    FETCH base ADD DRY RUN
    FETCH base ADD NO INDEX
