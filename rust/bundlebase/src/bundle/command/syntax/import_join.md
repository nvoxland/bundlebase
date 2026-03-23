Solidify an existing `bundle://` join by copying all commits, data files, indexes, and connectors/functions from the source bundle into the target. The join must have been created with `JOIN 'bundle://...'`.

### Examples

    IMPORT JOIN stations
    IMPORT JOIN stations FLATTEN HISTORY

`FLATTEN HISTORY` collapses all imported commits into a single commit. Without it, each source commit becomes a separate commit in the target (prefixed with `[import <name>]`).

Connectors and functions from the source are imported. An error is raised if any name conflicts with existing connectors or functions in the target.

Bundle-level metadata (name, description, config, views) from the source is NOT imported.
