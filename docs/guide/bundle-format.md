---
seo_title: "Bundle Format: What a Bundle Looks Like on Disk - Bundlebase"
description: A bundle is a plain directory of YAML manifests and data files. No server, no catalog database. This page shows every file type and what it holds.
---

# Bundle Format

A bundle is a directory. There is no server, no catalog database, and no lock file. Anything that can store files can hold a bundle: a local disk, S3, GCS, Azure Blob, or a tar archive. Copying the directory copies the bundle.

This page describes what is in that directory. You never need to edit these files, but knowing what they are explains why a bundle can be moved, shared, and audited with ordinary tools.

## A bundle on disk

This is a real bundle after four commits: attach a CSV, describe a column, delete some rows, and update a value.

```
sales/
├── _bundlebase/
│   ├── 00000000000000000.yaml           bundle identity
│   ├── 0000175aa42c324c5.yaml           commit: attach + normalize names
│   ├── 00003c6b2d27e0ada.yaml           commit: describe a column
│   ├── 00005392d7ee63696.yaml           commit: delete rows
│   └── 00007b021ed1d76da.yaml           commit: update a value
├── 5d/fac2a5a8ed1b0b.block.schema.yaml      schema of an attached file
├── 85/dd8f5860f1d48f.block.columns.yaml     stable column ids for that file
├── b1/bb977e3b2f2d33.block.layout.pagemap   row-to-byte map for fast lookups
├── a0/b206e3459a78ff.overlay.tomb.rowids    rows removed by the delete
└── d3/a61581e6b324c9.overlay.update.parquet values changed by the update
```

Everything outside `_bundlebase/` is content-addressed. A file's name is derived from a hash of its contents, so a file is never rewritten in place, and two bundles holding the same data hold the same file names.

## Manifests

`_bundlebase/` holds the history. The first file identifies the bundle:

```yaml
id: af192aac-d1b5-4e26-96b8-fa37292db660
minVersion: '0.13'
```

Each commit adds one more YAML file. The name is a five-digit sequence number followed by a hash of the content. Listing the directory in name order gives the commits in the order they were made.

A commit records who made it, when, why, and the exact operations it applied:

```yaml
author: nvoxland
message: Drop Africa
timestamp: 2026-10-14T03:57:15Z
changes:
- description: DELETE FROM bundle WHERE sales_region = 'Africa'
  operations:
  - type: delete
    tombstone: a0/b206e3459a78ff.overlay.tomb.rowids
    where: col_74ec8b3e36709b6a = 'Africa'
```

The recorded `where` names the column by its stable id rather than its current name, so the record stays accurate after a rename.

Opening a bundle reads these files in order and applies each operation. The current state of the bundle, including its schema, column descriptions, metrics, views, and cleanup rules, is the result of that replay. Nothing else stores it.

That is also why history is never lost. The manifests are the only source of truth, and they are only ever added to.

## Data files

**Attached data.** Data fetched by a source is copied into the bundle as Parquet, in files named like `64/d894c6a8ff0647.block.data.parquet`. A file you `ATTACH` directly from local disk, S3, GCS, or Azure is referenced in place by URL. Either way the bundle records a checksum, and `VERIFY DATA` checks it later, so a file that changed underneath the bundle is caught rather than silently read. A bundle whose data is all copied in is fully self-contained. One that references files in place travels only as far as those URLs are readable.

**Block sidecars.** Each attached file gets a small schema file and a column id file. Column ids are what keep descriptions, updates, and metrics attached to the right column after a rename. Some formats also get a layout file that maps row numbers to byte positions.

**Deletes and updates.** A `DELETE` does not rewrite data. It writes a small file listing the rows it removed, and the scan skips them. An `UPDATE` writes a Parquet file holding only the changed cells, keyed by row and column id, and the scan merges them in. Both are recorded in the commit that made them, so every change to a value can be traced back to a commit, an author, and the `WHERE` clause that selected it.

## Reading a bundle's history yourself

Every manifest is plain YAML, so standard tools work:

```bash
# Every commit message, in order
grep -h '^message:' sales/_bundlebase/0*.yaml

# Every file the bundle has ever attached
grep -h 'location:' sales/_bundlebase/0*.yaml
```

The same information is available as tables from inside bundlebase:

```sql
SHOW HISTORY;
SELECT id, num_rows, source_locations, source_versions FROM bundle_info.blocks;  -- for data fetched by a source
```

## What is standard and what is not

| Piece | Format |
|---|---|
| Copied data | Apache Parquet |
| Update overlays | Apache Parquet |
| Manifests and sidecars | YAML |
| Delete files | Bundlebase binary format |
| Layout files | Bundlebase binary format |

The data itself, and every changed value, is standard Parquet that any Arrow or Parquet reader can open.
