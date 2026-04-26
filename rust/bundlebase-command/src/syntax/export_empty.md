# EXPORT EMPTY

Creates an "empty" bundle at the target path — containing source definitions,
always-update/always-delete rules, column operations, and structure, but no
attached data. Recipients can open the empty bundle and run `FETCH` to pull
the raw data themselves.

## Syntax

```sql
EXPORT EMPTY TO '<path>'
```

The target path supports `.tar` files via the existing tar bundle support.

## Examples

```sql
-- Export to a directory
EXPORT EMPTY TO 'path/to/empty'

-- Export to a tar file (portable, single-file bundle)
EXPORT EMPTY TO 'path/to/empty.tar'

-- Absolute path
EXPORT EMPTY TO '/tmp/empty_bundle'
```

## Behavior

- Strips all data operations: `ATTACH`, `DETACH`, `REPLACE`, `DELETE`, `UPDATE`, `INDEX`
- Preserves: `CREATE SOURCE`, `ALWAYS DELETE`, `ALWAYS UPDATE`, `RENAME COLUMN`,
  `CAST COLUMN`, `ADD COLUMN`, `DROP COLUMN`, `FILTER`, `JOIN`, views, indexes definitions
- Fills the `EXPECTED SCHEMA` on each `CREATE SOURCE` from the last-seen fetched schema —
  so column operations continue to resolve correctly, and `FETCH` can validate the schema
- The empty bundle has no rows and no schema until `FETCH` is run
- History is reset to a single "Empty export" commit

## Workflow

```sql
-- Build a normal bundle with sources and transformations
CREATE SOURCE USING remote_dir WITH (url = 's3://bucket/data/')
FETCH base ADD
RENAME COLUMN old_name TO new_name
COMMIT 'initial'

-- Export the empty bundle for sharing
EXPORT EMPTY TO '/shared/empty'

-- Recipient opens empty bundle and fetches data
-- (in the empty bundle):
FETCH base ADD
```
