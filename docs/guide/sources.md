# Data Sources

Sources allow you to define where data files come from and automatically discover and attach new files as they become available. This is useful for working with directories of files that grow over time, such as daily data exports or streaming data partitions.

## Overview

The source workflow has two steps:

1. **Define a source** with `create_source()` - Specifies where to look for files
2. **Fetch new files** with `fetch()` - Discovers and attaches any new files found

## Basic Usage

=== "Async API"

    ```python
    import bundlebase as bb

    # Create a bundle with a source
    c = await (bb.create("my/data")
        .create_source("remote_dir", {
            "url": "s3://my-bucket/data/",
            "patterns": "**/*.parquet"
        }))

    # Fetch discovers and attaches all matching files
    await c.fetch()

    # Later, fetch again to get any new files
    await c.fetch()

    await c.commit("Added data from S3")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    # Create a bundle with a source
    c = (bb.create("my/data")
        .create_source("remote_dir", {
            "url": "s3://my-bucket/data/",
            "patterns": "**/*.parquet"
        }))

    # Fetch discovers and attaches all matching files
    c.fetch()

    # Later, fetch again to get any new files
    c.fetch()

    c.commit("Added data from S3")
    ```

## Source Functions

### remote_dir

Lists files from a local or cloud directory. Supports any URL scheme supported by the IO registry (S3, GCS, Azure, file://, etc.).

**Arguments:**

| Argument | Required | Description |
|----------|----------|-------------|
| `url` | Yes | Directory URL (e.g., `s3://bucket/data/`, `file:///path/to/data/`) |
| `patterns` | No | Comma-separated glob patterns (default: `**/*`) |
| `copy` | No | `"true"` to copy files into bundle (default), `"false"` to reference in place |

=== "Async API"

    ```python
    # S3 bucket
    c = await c.create_source("remote_dir", {
        "url": "s3://my-bucket/data/",
        "patterns": "**/*.parquet"
    })

    # Local directory
    c = await c.create_source("remote_dir", {
        "url": "file:///data/exports/",
        "patterns": "**/*.csv,**/*.parquet"
    })

    # Reference files in place instead of copying
    c = await c.create_source("remote_dir", {
        "url": "s3://my-bucket/data/",
        "copy": "false"
    })
    ```

=== "Sync API"

    ```python
    # S3 bucket
    c = c.create_source("remote_dir", {
        "url": "s3://my-bucket/data/",
        "patterns": "**/*.parquet"
    })

    # Local directory
    c = c.create_source("remote_dir", {
        "url": "file:///data/exports/",
        "patterns": "**/*.csv,**/*.parquet"
    })

    # Reference files in place instead of copying
    c = c.create_source("remote_dir", {
        "url": "s3://my-bucket/data/",
        "copy": "false"
    })
    ```

### ftp_directory

Lists files from an FTP server. Supports anonymous and authenticated access.

**Arguments:**

| Argument | Required | Description |
|----------|----------|-------------|
| `url` | Yes | FTP URL (e.g., `ftp://user:pass@host:21/path/`) |
| `patterns` | No | Comma-separated glob patterns (default: `**/*`) |

!!! note
    Files are always copied into the bundle since FTP URLs cannot be directly referenced during query execution.

=== "Async API"

    ```python
    # Anonymous FTP
    c = await c.create_source("ftp_directory", {
        "url": "ftp://ftp.example.com/pub/data/"
    })

    # Authenticated FTP
    c = await c.create_source("ftp_directory", {
        "url": "ftp://user:pass@ftp.example.com/data/",
        "patterns": "**/*.csv"
    })
    ```

=== "Sync API"

    ```python
    # Anonymous FTP
    c = c.create_source("ftp_directory", {
        "url": "ftp://ftp.example.com/pub/data/"
    })

    # Authenticated FTP
    c = c.create_source("ftp_directory", {
        "url": "ftp://user:pass@ftp.example.com/data/",
        "patterns": "**/*.csv"
    })
    ```

### sftp_directory

Lists files from a remote directory via SFTP. Requires an SSH private key for authentication.

**Arguments:**

| Argument | Required | Description |
|----------|----------|-------------|
| `url` | Yes | SFTP URL (e.g., `sftp://user@host:22/path/`) |
| `key_path` | Yes | Path to SSH private key file (e.g., `~/.ssh/id_rsa`) |
| `patterns` | No | Comma-separated glob patterns (default: `**/*`) |

!!! note
    Files are always copied into the bundle since SFTP URLs cannot be directly referenced during query execution.

=== "Async API"

    ```python
    c = await c.create_source("sftp_directory", {
        "url": "sftp://user@host/data/",
        "key_path": "~/.ssh/id_rsa",
        "patterns": "**/*.parquet"
    })
    ```

=== "Sync API"

    ```python
    c = c.create_source("sftp_directory", {
        "url": "sftp://user@host/data/",
        "key_path": "~/.ssh/id_rsa",
        "patterns": "**/*.parquet"
    })
    ```

## Fetching Data

### fetch()

Discovers and attaches new files from a specific pack's sources. Returns a list of `FetchResults`, one for each source.

=== "Async API"

    ```python
    # Fetch from base pack (default)
    results = await c.fetch()
    for result in results:
        print(f"{result.source_function}: {len(result.added)} added")

    # Fetch from a joined pack
    results = await c.fetch("customers")
    ```

=== "Sync API"

    ```python
    # Fetch from base pack (default)
    results = c.fetch()
    for result in results:
        print(f"{result.source_function}: {len(result.added)} added")

    # Fetch from a joined pack
    results = c.fetch("customers")
    ```

### fetch_all()

Discovers and attaches new files from all defined sources across all packs. Returns a list of `FetchResults`, one for each source (including sources with no changes).

=== "Async API"

    ```python
    results = await c.fetch_all()
    for result in results:
        print(f"{result.pack}/{result.source_function}: {result.total_count()} changes")
    ```

=== "Sync API"

    ```python
    results = c.fetch_all()
    for result in results:
        print(f"{result.pack}/{result.source_function}: {result.total_count()} changes")
    ```

### FetchResults

Each `FetchResults` object contains:

| Property | Type | Description |
|----------|------|-------------|
| `source_function` | `str` | Source function name (e.g., "remote_dir") |
| `source_url` | `str` | Source URL |
| `pack` | `str` | Pack name ("base" or join name) |
| `added` | `list[FetchedBlock]` | Blocks that were newly added |
| `replaced` | `list[FetchedBlock]` | Blocks that were replaced (updated) |
| `removed` | `list[str]` | Source locations of blocks that were removed |

Methods:

- `total_count()` - Total number of changes (added + replaced + removed)
- `is_empty()` - Returns `True` if no changes were made

## Sources with Joins

You can define sources for joined packs by specifying the `pack` parameter.

=== "Async API"

    ```python
    import bundlebase as bb

    # Create bundle with base data
    c = await bb.create("my/data").attach("orders.parquet")

    # Create a join for customer data
    c = await c.join("customers", "base.customer_id = customers.id")

    # Define a source for the customers pack
    c = await c.create_source("remote_dir", {
        "url": "s3://bucket/customers/",
        "patterns": "**/*.parquet"
    }, pack="customers")

    # Fetch will attach files to the customers join
    results = await c.fetch("customers")
    print(f"Added {len(results[0].added)} customer files")

    await c.commit("Added customers from S3")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    # Create bundle with base data
    c = bb.create("my/data").attach("orders.parquet")

    # Create a join for customer data
    c = c.join("customers", "base.customer_id = customers.id")

    # Define a source for the customers pack
    c = c.create_source("remote_dir", {
        "url": "s3://bucket/customers/",
        "patterns": "**/*.parquet"
    }, pack="customers")

    # Fetch will attach files to the customers join
    results = c.fetch("customers")
    print(f"Added {len(results[0].added)} customer files")

    c.commit("Added customers from S3")
    ```

## Pattern Matching

The `patterns` argument accepts comma-separated glob patterns:

| Pattern | Matches |
|---------|---------|
| `**/*` | All files recursively (default) |
| `*.parquet` | Parquet files in the root directory |
| `**/*.parquet` | Parquet files in any subdirectory |
| `**/*.csv,**/*.parquet` | CSV and Parquet files |
| `2024/**/*.parquet` | Parquet files under the 2024 directory |

## Workflow Example

A typical workflow for incrementally loading data:

=== "Async API"

    ```python
    import bundlebase as bb

    # Initial setup
    c = await (bb.create("sales/data")
        .create_source("remote_dir", {
            "url": "s3://company/sales/",
            "patterns": "**/*.parquet"
        }))

    # Initial load
    results = await c.fetch()
    total_added = sum(len(r.added) for r in results)
    print(f"Initial load: {total_added} files")
    await c.commit("Initial data load")

    # ... time passes, new files appear in S3 ...

    # Incremental load (only attaches new files)
    c = await bb.open("sales/data")
    results = await c.fetch()
    total_added = sum(len(r.added) for r in results)
    if total_added > 0:
        print(f"Loaded {total_added} new files")
        await c.commit("Incremental data load")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    # Initial setup
    c = (bb.create("sales/data")
        .create_source("remote_dir", {
            "url": "s3://company/sales/",
            "patterns": "**/*.parquet"
        }))

    # Initial load
    results = c.fetch()
    total_added = sum(len(r.added) for r in results)
    print(f"Initial load: {total_added} files")
    c.commit("Initial data load")

    # ... time passes, new files appear in S3 ...

    # Incremental load (only attaches new files)
    c = bb.open("sales/data")
    results = c.fetch()
    total_added = sum(len(r.added) for r in results)
    if total_added > 0:
        print(f"Loaded {total_added} new files")
        c.commit("Incremental data load")
    ```
