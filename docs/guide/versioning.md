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

View uncommitted changes in the bundle.

=== "Async API"

    ```python
    status = bundle.status()
    print(f"Pending changes: {status.change_count}")
    ```

=== "Sync API"

    ```python
    status = bundle.status()
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

=== "Sync API"

    ```python
    bundle.attach("wrong_file.csv")
    bundle.reset()  # Discards the attach
    ```

=== "SQL"

    ```sql
    RESET
    ```

## Undo

Undo the last uncommitted operation. Can be called multiple times to undo several operations.

=== "Async API"

    ```python
    await bundle.attach("a.csv")
    await bundle.attach("b.csv")
    await bundle.undo()  # Undoes the attach of b.csv
    ```

=== "Sync API"

    ```python
    bundle.attach("a.csv")
    bundle.attach("b.csv")
    bundle.undo()  # Undoes the attach of b.csv
    ```

=== "SQL"

    ```sql
    UNDO
    ```

## Verify Data

Verify the integrity of all files in the bundle by checking SHA256 hashes.

=== "Async API"

    ```python
    results = await bundle.verify_data()
    print(results)

    # Optionally update stored version metadata if hashes match
    results = await bundle.verify_data(update_versions=True)
    ```

=== "Sync API"

    ```python
    results = bundle.verify_data()
    print(results)

    results = bundle.verify_data(update_versions=True)
    ```

=== "SQL"

    ```sql
    VERIFY DATA

    VERIFY DATA UPDATE
    ```

## Manifest System

Bundlebase uses a versioned manifest system for tracking commits:

- Location: `{data_dir}/_bundlebase/` directory
- Format: YAML files with 5-digit version + 12-character hash
- Example: `00001a1b2c3d4e5f6.yaml`
- Each commit can contain multiple "changes" which are the modifications to the bundle

### Example commit file

```yaml
author: nvoxland
message: Indexed
timestamp: 2026-01-08T07:24:18Z
changes:
  - id: 11675b7b-8aca-4187-a38d-bfda4d739e0e
    description: Attach file:///path/to/customers-0-100.csv
    operations:
      - type: definePack
        id: '56'
      - type: attachBlock
        source: file:///path/to/customers-0-100.csv
        version: 846acd3-64650a58fdd47-4308
        id: '09'
        packId: '56'
```
