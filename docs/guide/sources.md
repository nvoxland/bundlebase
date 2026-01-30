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
    bundle = await (bb.create("my/data")
        .create_source("remote_dir", {
            "url": "s3://my-bucket/data/",
            "patterns": "**/*.parquet"
        }))

    # Fetch discovers and attaches all matching files
    await bundle.fetch()

    # Later, fetch again to get any new files
    await bundle.fetch()
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    # Create a bundle with a source
    bundle = (bb.create("my/data")
        .create_source("remote_dir", {
            "url": "s3://my-bucket/data/",
            "patterns": "**/*.parquet"
        }))

    # Fetch discovers and attaches all matching files
    bundle.fetch()

    # Later, fetch again to get any new files
    bundle.fetch()
    ```

=== "SQL"

    ```sql
    CREATE SOURCE remote_dir WITH (url = 's3://my-bucket/data/', patterns = '**/*.parquet')
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
    bundle = await bundle.create_source("remote_dir", {
        "url": "s3://my-bucket/data/",
        "patterns": "**/*.parquet"
    })

    # Local directory
    bundle = await bundle.create_source("remote_dir", {
        "url": "file:///data/exports/",
        "patterns": "**/*.csv,**/*.parquet"
    })

    # Reference files in place instead of copying
    bundle = await bundle.create_source("remote_dir", {
        "url": "s3://my-bucket/data/",
        "copy": "false"
    })
    ```

=== "Sync API"

    ```python
    # S3 bucket
    bundle = bundle.create_source("remote_dir", {
        "url": "s3://my-bucket/data/",
        "patterns": "**/*.parquet"
    })

    # Local directory
    bundle = bundle.create_source("remote_dir", {
        "url": "file:///data/exports/",
        "patterns": "**/*.csv,**/*.parquet"
    })

    # Reference files in place instead of copying
    bundle = bundle.create_source("remote_dir", {
        "url": "s3://my-bucket/data/",
        "copy": "false"
    })
    ```

=== "SQL"

    ```sql
    CREATE SOURCE remote_dir WITH (url = 's3://my-bucket/data/', patterns = '**/*.parquet')
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
    bundle = await bundle.create_source("ftp_directory", {
        "url": "ftp://ftp.example.com/pub/data/"
    })

    # Authenticated FTP
    bundle = await bundle.create_source("ftp_directory", {
        "url": "ftp://user:pass@ftp.example.com/data/",
        "patterns": "**/*.csv"
    })
    ```

=== "Sync API"

    ```python
    # Anonymous FTP
    bundle = bundle.create_source("ftp_directory", {
        "url": "ftp://ftp.example.com/pub/data/"
    })

    # Authenticated FTP
    bundle = bundle.create_source("ftp_directory", {
        "url": "ftp://user:pass@ftp.example.com/data/",
        "patterns": "**/*.csv"
    })
    ```

=== "SQL"

    ```sql
    CREATE SOURCE ftp_directory WITH (url = 'ftp://ftp.example.com/pub/data/')
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
    bundle = await bundle.create_source("sftp_directory", {
        "url": "sftp://user@host/data/",
        "key_path": "~/.ssh/id_rsa",
        "patterns": "**/*.parquet"
    })
    ```

=== "Sync API"

    ```python
    bundle = bundle.create_source("sftp_directory", {
        "url": "sftp://user@host/data/",
        "key_path": "~/.ssh/id_rsa",
        "patterns": "**/*.parquet"
    })
    ```

=== "SQL"

    ```sql
    CREATE SOURCE sftp_directory WITH (url = 'sftp://user@host/data/', key_path = '~/.ssh/id_rsa', patterns = '**/*.parquet')
    ```

## Fetching Data

### fetch()

Discovers and attaches new files from a specific pack's sources. Returns a list of `FetchResults`, one for each source.

=== "Async API"

    ```python
    # Fetch from base pack (default)
    results = await bundle.fetch()
    for result in results:
        print(f"{result.source_function}: {len(result.added)} added")

    # Fetch from a joined pack
    results = await bundle.fetch("customers")
    ```

=== "Sync API"

    ```python
    # Fetch from base pack (default)
    results = bundle.fetch()
    for result in results:
        print(f"{result.source_function}: {len(result.added)} added")

    # Fetch from a joined pack
    results = bundle.fetch("customers")
    ```

=== "SQL"

    ```sql
    FETCH

    FETCH customers
    ```

### fetch_all()

Discovers and attaches new files from all defined sources across all packs. Returns a list of `FetchResults`, one for each source (including sources with no changes).

=== "Async API"

    ```python
    results = await bundle.fetch_all()
    for result in results:
        print(f"{result.pack}/{result.source_function}: {result.total_count()} changes")
    ```

=== "Sync API"

    ```python
    results = bundle.fetch_all()
    for result in results:
        print(f"{result.pack}/{result.source_function}: {result.total_count()} changes")
    ```

=== "SQL"

    ```sql
    FETCH ALL
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
    bundle = await bb.create("my/data").attach("orders.parquet")

    # Create a join for customer data
    bundle = await bundle.join("customers", "base.customer_id = customers.id")

    # Define a source for the customers pack
    bundle = await bundle.create_source("remote_dir", {
        "url": "s3://bucket/customers/",
        "patterns": "**/*.parquet"
    }, pack="customers")

    # Fetch will attach files to the customers join
    results = await bundle.fetch("customers")
    print(f"Added {len(results[0].added)} customer files")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    # Create bundle with base data
    bundle = bb.create("my/data").attach("orders.parquet")

    # Create a join for customer data
    bundle = bundle.join("customers", "base.customer_id = customers.id")

    # Define a source for the customers pack
    bundle = bundle.create_source("remote_dir", {
        "url": "s3://bucket/customers/",
        "patterns": "**/*.parquet"
    }, pack="customers")

    # Fetch will attach files to the customers join
    results = bundle.fetch("customers")
    print(f"Added {len(results[0].added)} customer files")
    ```

=== "SQL"

    ```sql
    CREATE SOURCE remote_dir WITH (url = 's3://bucket/customers/', patterns = '**/*.parquet') ON customers
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
    bundle = await (bb.create("sales/data")
        .create_source("remote_dir", {
            "url": "s3://company/sales/",
            "patterns": "**/*.parquet"
        }))

    # Initial load
    results = await bundle.fetch()
    total_added = sum(len(r.added) for r in results)
    print(f"Initial load: {total_added} files")
    await bundle.commit("Initial data load")

    # ... time passes, new files appear in S3 ...

    # Incremental load (only attaches new files)
    bundle = await bb.open("sales/data")
    results = await bundle.fetch()
    total_added = sum(len(r.added) for r in results)
    if total_added > 0:
        print(f"Loaded {total_added} new files")
        await bundle.commit("Incremental data load")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    # Initial setup
    bundle = (bb.create("sales/data")
        .create_source("remote_dir", {
            "url": "s3://company/sales/",
            "patterns": "**/*.parquet"
        }))

    # Initial load
    results = bundle.fetch()
    total_added = sum(len(r.added) for r in results)
    print(f"Initial load: {total_added} files")
    bundle.commit("Initial data load")

    # ... time passes, new files appear in S3 ...

    # Incremental load (only attaches new files)
    bundle = bb.open("sales/data")
    results = bundle.fetch()
    total_added = sum(len(r.added) for r in results)
    if total_added > 0:
        print(f"Loaded {total_added} new files")
        bundle.commit("Incremental data load")
    ```
