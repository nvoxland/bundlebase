---
seo_title: "Versioning: Commits, History, and Rollback for Data - Bundlebase"
---

# Versioning

Bundlebase has a built-in versioning system. Every time you call `commit()`, a new version is created with all your changes.

Until `commit()` is called, any changes you have made remain in-memory and only apply to your bundle instance. **To share changes, you must commit.**

## Committing Changes

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.create("my/data")
    await bundle.attach("customers.csv")
    await bundle.commit("Added customer data")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data")
    bundle.attach("customers.csv")
    bundle.commit("Added customer data")
    ```

=== "SQL"

    ```sql
    COMMIT 'Added customer data'
    ```

## Viewing Version and History

=== "Async API"

    ```python
    # Current version identifier
    print(bundle.version)

    # Full commit history
    for entry in bundle.history():
        print(entry)
    ```

=== "Sync API"

    ```python
    print(bundle.version)

    for entry in bundle.history():
        print(entry)
    ```

=== "SQL"

    ```sql
    SELECT * FROM bundle_info.history
    ```

    or 

    ```
    SHOW HISTORY
    ```

## Status

View uncommitted changes in the bundle. `status()` is available on builders
(from `create()` or `extend()`) — a bundle opened read-only has no uncommitted
changes and no `status()` method.

=== "Async API"

    ```python
    builder = await bundlebase.create("./my_bundle")
    await builder.attach("data.parquet")
    status = builder.status()
    print(f"Pending changes: {status.change_count}")
    ```

=== "Sync API"

    ```python
    builder = bundlebase.sync.create("./my_bundle")
    builder.attach("data.parquet")
    status = builder.status()
    print(f"Pending changes: {status.change_count}")
    ```

=== "SQL"

    ```sql
    SELECT * FROM bundle_info.status
    ```

    or

    ```
    SHOW STATUS
    ```

## Reset

Discard all uncommitted changes, reverting to the last committed state.

=== "Async API"

    ```python
    await bundle.attach("wrong_file.csv")
    await bundle.reset()  # Discards the attach
    ```

=== "SQL"

    ```sql
    RESET
    ```

!!! note
    `reset()` and `undo()` are not available in the sync API yet.

## Undo

Undo the last uncommitted operation. Can be called multiple times to undo several operations, or use `UNDO LAST N` to undo multiple at once.

=== "Async API"

    ```python
    await bundle.attach("a.csv")
    await bundle.attach("b.csv")
    await bundle.undo()  # Undoes the attach of b.csv
    ```

=== "SQL"

    ```sql
    UNDO
    UNDO LAST 3
    ```

## Verify Data

Verify the integrity of all files in the bundle by checking SHA256 hashes.

=== "Async API"

    ```python
    results = await bundle.verify_data()
    print(results)

    # On builders: optionally update stored version metadata if hashes match
    results = await builder.verify_data(update_versions=True)
    ```

=== "Sync API"

    ```python
    results = bundle.verify_data()
    print(results)

    # On builders: optionally update stored version metadata if hashes match
    results = builder.verify_data(update_versions=True)
    ```

=== "SQL"

    ```sql
    VERIFY DATA

    VERIFY DATA UPDATE
    ```

!!! note
    `update_versions=True` (`VERIFY DATA UPDATE` in SQL) requires a builder
    (`create()`/`extend()`); bundles opened read-only can verify but not update
    version metadata.

## Format Compatibility

Every bundle records the minimum bundlebase version required to open it. This is set automatically when a bundle is created and tracks the major.minor version (e.g., `0.9`). A maximum version can also be recorded, but new bundles leave it unset so any future bundlebase version can open them.

When opening a bundle:

- If the current bundlebase version is **below** the bundle's minimum version, an error is returned. This means the bundle uses features that this version of bundlebase doesn't support.
- If the bundle has a maximum version recorded and the current bundlebase version is **above** it, an error is returned with a suggestion to upgrade the bundle.

Old bundles created before version tracking was added will open without restriction.

### Upgrading a Bundle

When you update bundlebase to a newer version, existing bundles may need their version range updated.

=== "Async API"

    ```python
    import bundlebase as bb

    await bb.upgrade_bundle("/path/to/bundle")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bb.upgrade_bundle("/path/to/bundle")
    ```

=== "CLI"

    ```bash
    bundlebase upgrade-bundle --bundle /path/to/bundle
    ```

=== "MCP"

    Use the `upgrade_bundle` tool with the bundle path.

This updates the bundle's min and max version to match the current bundlebase version, committing the change. The upgrade writes directly to the bundle's manifest without opening it, so it works even when the version mismatch would prevent a normal open.

## Manifest System

Each commit is stored as one YAML file under `_bundlebase/` in the bundle directory, recording the author, message, timestamp, and every operation applied. See [Bundle Format](bundle-format.md) for the full layout and an annotated example.
