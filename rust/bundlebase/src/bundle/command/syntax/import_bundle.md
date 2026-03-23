Import an entire bundle as a join pack, copying all commits, data files, indexes, and connectors/functions. Operations referencing the source base pack are remapped to the new join pack.

### Examples

    IMPORT BUNDLE './stations' AS stations ON lake_id = stations.lake_id
    IMPORT BUNDLE './stations' FLATTEN HISTORY AS stations ON lake_id = stations.lake_id

`FLATTEN HISTORY` collapses all imported commits into a single commit. Without it, each source commit becomes a separate commit in the target (prefixed with `[import <name>]`).

Connectors and functions from the source are imported. An error is raised if any name conflicts with existing connectors or functions in the target.

Bundle-level metadata (name, description, config, views) from the source is NOT imported.
