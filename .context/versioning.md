# Commit-Based Versioning

## Manifest System

Bundlebase uses a versioned manifest system for persistence:

**Manifest storage:**
- Location: `{data_dir}/_bundlebase/` directory
- Format: YAML files named `{5-digit-version}{12-char-hash}.yaml`
- Example: `00001abc123def456.yaml` (version 1), `00042xyz789abc123.yaml` (version 42)
- Init commit: `00000000000000000.yaml` (created when the bundle is created)

**Numbering and replay order:**
- `commit()` lists `_bundlebase/` at write time and uses `max(highest listed number, highest number this session loaded or wrote) + 1`. The in-memory term covers a listing that lags the session's own writes on an eventually-consistent store. The upgrade path uses the same helper (`commit::next_manifest_version`).
- Concurrent writers can produce the same number. That is expected: the hash suffix keeps the names distinct, and there is no compare-and-swap step on any backend.
- Replay order is the lexicographic order of the full filename (`commit::list_manifest_files`). The zero-padded number sorts first and the hash breaks ties, so every backend replays the same set of files in the same order.
- Only files directly in `_bundlebase/` are commits. The init file and anything in a subdirectory are skipped.
- Known limit: the five-digit prefix stops sorting correctly after 99999 commits to one directory.

**Init commit structure** (`00000000000000000.yaml`):
```yaml
id: 4f8a1c2e-...        # Bundle UUID (only when creating, not extending)
from: file:///base      # Optional: parent bundle URL ('from' chain)
minVersion: "0.13"      # Minimum bundlebase format version required to open
# maxVersion is left unset so future bundlebase versions can open the bundle
```

**Commit structure** (`00001....yaml`, written by `commit()`):
```yaml
author: nathan
message: Initial data load
timestamp: 2024-01-15T10:30:00Z
changes:
  - id: 12345678-1234-...
    description: ATTACH data.parquet
    operations:
      - type: attachBlock
        # ... operation-specific config
```

## Format Version Requirements

Bundles record which bundlebase versions can read them:

- **Min version** is set automatically when a bundle is **created** (stored as
  `major.minor` of the creating bundlebase version), so older clients get a
  clear upgrade message.
- **Max version** is NOT auto-set, so any future bundlebase version can open
  the bundle without a bundle upgrade.
- `SET MIN VERSION` / `SET MAX VERSION` commands (`SetMinVersionOp` /
  `SetMaxVersionOp`) adjust the requirements explicitly.
- `bundlebase upgrade-bundle` upgrades a bundle's format version to the
  current bundlebase version.

## Persisted DELETE and UPDATE edits

DELETE edits are stored in one content-addressed Puffin file per commit that
contains pending deletes. The file uses the existing sharded `ContentAddress`
layout and the `.overlay.tomb.puffin` suffix. It contains one standard
deletion-vector blob per affected `(BlockId, version)`. Each blob carries the
stable block ID and version as Bundlebase properties, and stores deleted row
positions in the standard Puffin deletion-vector encoding. `DeleteOp` keeps
the manifest fields `tombstone` and `where`; `where` records the predicate for
history.

On replay, vectors are unioned for the matching block version. A detached or
replaced version that is no longer live is skipped, so a delete cannot attach
to different content that reused the block ID. UPDATE remains a Parquet
overlay keyed by the same stable block version; when concurrent commits update
the same cell, the last overlay in manifest filename replay order wins.

Bundles written with the previous tombstone container have no migration or
legacy-reader path. Opening one must report an unsupported tombstone format.
Missing or corrupt deletion files also fail replay clearly rather than
silently restoring deleted rows. Puffin standardization and Roaring encoding
may reduce delete-file size, but make no query-performance promise. This does
not implement checkpoints or make bundles Iceberg tables.

## Commit Workflow

```python
# Create mutable container
c = await Bundlebase.create("/my/container")

# Make modifications
await c.attach("data.parquet")
await c.drop_column("sensitive")

# Commit creates versioned snapshot
await c.commit("Cleaned sensitive data")
# Creates: /my/container/_bundlebase/{version}-{hash}.yaml

# Later, open the container
c = await Bundlebase.open("/my/container")
# Loads the latest manifest and reconstructs container state
```

## From Chain

Containers can extend other containers to build on previous versions:

```python
# Load committed container
base = await Bundlebase.open("/base/container")

# Extend to new directory with new modifications
extended = await base.extend("/extended/container")
await extended.attach("new_data.parquet")
await extended.commit("Added new data")

# Results in manifest:
# {
#   "from": "/base/container",
#   "operations": [...]
# }
```

**Benefits:**
- **Version history**: All commits are preserved
- **Branching**: Can extend from any previous commit
- **Relative paths**: Manifests support relative paths for portability
- **Circular dependency detection**: Prevents invalid from chains

## Replay Checkpoints

Checkpoints in `_bundlebase/checkpoints/` are optional caches of an exact ordered
manifest prefix. Original manifests remain authoritative and are never deleted
by checkpoint maintenance. Checkpoint files preserve the complete original
commits and operations for public history, alongside a separate replay list.
Only consecutive DELETE/UPDATE runs are consolidated into content-addressed edit
sidecars. Every other operation is a replay barrier; ATTACH and metadata
operations are not blindly removed or deduplicated.

Selection validates the format, checksum, cache filename, exact listed manifest
prefix, and coverage/history consistency. An init/parent dependency digest binds
the cache to the init commit and the parent's full committed head, including view
parents. A late manifest inside the covered prefix invalidates the cache.
Invalid or unusable candidates fall back to another checkpoint or full replay.

The committed version chain folds each original manifest's persisted operations
into a full 32-byte digest. The public version exposes its first 12 hexadecimal
characters. Checkpoints save and restore the full digest; consolidated replay
operations never replace original operations in this chain.
This replaces the previous flattened-operation hash, so existing bundles receive
new version strings on open. Manifests are unchanged and require no migration.
For the same canonical ordered commit set, the version is identical whether
loaded from full history or a valid checkpoint plus its tail. Adding, removing,
or replacing checkpoints does not change it. A late commit inside a covered
prefix invalidates that checkpoint; a checkpoint ending before the insertion
can still seed replay of the newly ordered tail. A newly discovered commit
changes the commit set, so it can change the version, consistently in both modes.

`checkpoint.every=50` and `checkpoint.max_age=3d` are the default automatic
count/age thresholds. `checkpoint.grace=1h` leaves recent commits uncovered;
coverage stops at the first ineligible commit to retain a prefix. `every=0`
disables automatic creation. Runtime-only `checkpoint.enabled=false` disables
checkpoint loading and automatic creation for diagnostic full replay.

`CHECKPOINT` creates a cache of currently durable manifests without committing
pending changes, bypassing automatic count/age/grace checks. Creation retains
the newest two checkpoint files by descending filename. `CHECKPOINT PRUNE`
retains the newest one by that ordering.
Neither records an operation. Pruning never removes manifests or edit sidecars.
Automatic maintenance runs after durable commit publication and warns on failure
rather than failing an already successful commit; explicit commands return errors.

Checkpoints reduce manifest reads and repeated edit-file loads. Their original
history metadata still grows with history, and arbitrary operations still replay;
there is no constant-time opening guarantee. Sidecar reachability and garbage
collection are separate work.

**Decision history:** October 2026: adopted exact-prefix, dependency-validated
caches with separate history and replay representations to preserve history,
operation ordering, and version identity. See
[ADR-014](decisions/014-self-validating-checkpoints.md) and the end-user
[maintenance guide](../docs/guide/maintenance.md).

## Path Handling

### Path Resolution

`attach()` resolves paths through `bundlebase_io::readable_file_from_path`:

- Absolute filesystem paths (`"/full/path/to/sales.csv"`) become local `file://`
  URLs, regardless of the bundle's storage protocol. URL conversion escapes
  spaces, literal percent signs, and other special characters.
- Explicit URLs (`"file:///full/path/to/sales.csv"`, `"s3://bucket/sales.csv"`)
  retain their protocol and location.
- Relative paths (`"data/sales.csv"`) resolve within the bundle's data directory,
  retaining its storage protocol. Their persisted relative representation remains
  portable with the bundle; they are not canonicalized against the process's
  working directory.

Absolute path conversion does not require the file to exist; normal attachment
validation reports missing files. Relative directory/file helpers keep their
existing joining behavior.

**Test data location:**
- All test data in `test_data/` directory at project root
- Rust tests: `"test_data/userdata.parquet"`
- Python tests: `"test_data/userdata.parquet"` (pytest runs from project root)

## Error Handling

**Current implementation:**
- Python bindings use `.map_err()` to convert Rust errors to Python exceptions
- Operation validation happens in the `check()` phase (immediate feedback)
- State modification in `reconfigure()` phase validates preconditions
- DataFrame transformation errors surface during `apply_dataframe()`

**Error flow:**

```python
c = await Bundlebase.create("memory:///test_container")
await c.attach("data.parquet")

# Validation happens immediately (check phase)
try:
    await c.drop_column("nonexistent")  # May raise error here
except Exception as e:
    print(f"Validation error: {e}")

# If validation passes, operation is recorded
await c.filter("invalid_column > 5")  # May pass check, fail on query

# Execution errors surface during query
try:
    results = await c.to_dict()  # DataFrame transformation error
except Exception as e:
    print(f"Execution error: {e}")
```

**Error types:**
- **Validation errors**: Column not found, invalid schema, type mismatches
- **I/O errors**: File not found, permission denied, network errors
- **Serialization errors**: Invalid manifest format, version mismatch
- **Execution errors**: DataFusion query errors, missing data

**Best practices:**
1. Check container schema before operations: `c.schema.field("column_name")`
2. Validate data types before operations
3. Handle errors from async operations with try/except
4. Use descriptive error messages for debugging
