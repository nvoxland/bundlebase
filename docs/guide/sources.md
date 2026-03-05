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
    await bundle.fetch("base", "add")

    # Later, fetch again to get any new files
    await bundle.fetch("base", "add")
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
    bundle.fetch("base", "add")

    # Later, fetch again to get any new files
    bundle.fetch("base", "add")
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

### kaggle

Downloads dataset files from [Kaggle](https://www.kaggle.com/) via the Kaggle REST API. Discovers individual files within a dataset, downloads them as ZIP archives, and extracts the contents automatically.

**Arguments:**

| Argument | Required | Description |
|----------|----------|-------------|
| `dataset` | Yes | Dataset identifier in `owner/dataset-name` format (e.g., `zillow/zecon`) |
| `patterns` | No | Comma-separated glob patterns (default: `**/*`) |
| `mode` | No | Sync mode: `add` (default), `update`, or `sync` |
| `version` | No | Dataset version number to download (default: latest) |

!!! note "Authentication"
    Kaggle credentials are configured via [bundlebase configuration](configuration.md). Available config keys for the `kaggle` scope:

    | Key | Description | Default |
    |-----|-------------|---------|
    | `username` | Kaggle username | from `~/.kaggle/kaggle.json` |
    | `key` | Kaggle API key | from `~/.kaggle/kaggle.json` |
    | `url` | Kaggle API base URL | `https://www.kaggle.com` |

    If `username` and `key` are not set via bundlebase config, they fall back to the standard Kaggle credentials file at `~/.kaggle/kaggle.json`. You can create this file by running `kaggle` CLI setup or by generating an API token from your [Kaggle account settings](https://www.kaggle.com/settings).

!!! note
    Files are always copied into the bundle since Kaggle files are downloaded from a remote API.

=== "Async API"

    ```python
    # All files from a dataset
    bundle = await bundle.create_source("kaggle", {
        "dataset": "zillow/zecon"
    })

    # Only CSV files
    bundle = await bundle.create_source("kaggle", {
        "dataset": "zillow/zecon",
        "patterns": "*.csv"
    })

    # With sync mode to detect updates
    bundle = await bundle.create_source("kaggle", {
        "dataset": "zillow/zecon",
        "mode": "update"
    })
    ```

=== "Sync API"

    ```python
    # All files from a dataset
    bundle = bundle.create_source("kaggle", {
        "dataset": "zillow/zecon"
    })

    # Only CSV files
    bundle = bundle.create_source("kaggle", {
        "dataset": "zillow/zecon",
        "patterns": "*.csv"
    })

    # With sync mode to detect updates
    bundle = bundle.create_source("kaggle", {
        "dataset": "zillow/zecon",
        "mode": "update"
    })
    ```

=== "SQL"

    ```sql
    CREATE SOURCE kaggle WITH (dataset = 'zillow/zecon', patterns = '*.csv')
    ```

### Custom Source Functions

Bundlebase supports two modes for custom source functions. Custom sources use the three-step API: `create_connector()`, `set_connector_logic()` or `set_temporary_connector_logic()`, and `create_source()`.

- **`set_connector_logic()`** — Persists the logic into the bundle (creates an operation in commit history). Use for portable, cross-platform bundles. Rejects `type_='python'` since Python code can't be bundled.
- **`set_temporary_connector_logic()`** — Sets logic at runtime only (no operation persisted). Use for `type_='python'` in-process sources. Works on both `Bundle` (read-only) and `BundleBuilder`.

#### native (In-Process, Zero-Copy)

Loads a source function in-process for zero-copy Arrow data transfer. Best for performance-critical pipelines.

**Source logic arguments:**

| Argument | Required | Description |
|----------|----------|-------------|
| `type_` | Yes | `'python'` (Python in-process) or `'lib'` (shared library) |
| `logic` | Yes | `module:Class` (for `python`) or path to shared library (for `lib`) |
| `platform` | No | Target platform (e.g., `linux/amd64`, `darwin/arm64`, `*/*` default) |

=== "Python Native"

    ```python
    bundle.create_connector('example.connector')
    bundle.set_temporary_connector_logic('example.connector', type_='python', logic='example_connector:ExampleConnector')
    bundle.create_source('example.connector')
    ```

=== "Shared Library (Persisted)"

    ```python
    bundle.create_connector('example.connector')
    bundle.set_connector_logic('example.connector', type_='lib', logic='./target/release/libexample_connector.so')
    bundle.create_source('example.connector')
    ```

=== "SQL (Temporary)"

    ```sql
    CREATE CONNECTOR example.connector
    SET TEMPORARY CONNECTOR LOGIC example.connector WITH (type = 'python', logic = 'example_connector:ExampleConnector', platform = '*/*')
    CREATE SOURCE example.connector
    ```

=== "SQL (Persisted)"

    ```sql
    CREATE CONNECTOR example.connector
    SET CONNECTOR LOGIC example.connector WITH (type = 'lib', logic = './target/release/libexample_connector.so', platform = '*/*')
    CREATE SOURCE example.connector
    ```

#### ipc (Subprocess)

Delegates data discovery and retrieval to an external subprocess. This lets you write source functions in any language that speaks the JSON-RPC 2.0 + Arrow IPC protocol.

**Source logic arguments:**

| Argument | Required | Description |
|----------|----------|-------------|
| `type_` | Yes | `'ipc'`, `'java'`, or `'docker'` |
| `logic` | Yes | Command or path to run (see [type values](custom-sources/#type-values)) |
| `platform` | No | Target platform (e.g., `linux/amd64`, `darwin/arm64`, `*/*` default) |

=== "Async API"

    ```python
    bundle = await bundle.create_connector('example.connector')
    bundle = await bundle.set_connector_logic('example.connector', type_='ipc', logic='./example_connector')
    bundle = await bundle.create_source('example.connector')
    ```

=== "Sync API"

    ```python
    bundle.create_connector('example.connector')
    bundle.set_connector_logic('example.connector', type_='ipc', logic='./example_connector')
    bundle.create_source('example.connector')
    ```

=== "SQL"

    ```sql
    CREATE CONNECTOR example.connector
    SET CONNECTOR LOGIC example.connector WITH (type = 'ipc', logic = './example_connector')
    CREATE SOURCE example.connector
    ```

See [Custom Source Functions](custom-sources/) for SDKs, full examples, and protocol reference.

### Dropping a Connector

To completely remove a connector and all its associated logic, use `drop_connector()`.

=== "Async API"

    ```python
    bundle = await bundle.drop_connector('example.connector')
    ```

=== "Sync API"

    ```python
    bundle.drop_connector('example.connector')
    ```

=== "SQL"

    ```sql
    DROP CONNECTOR example.connector
    ```

This removes the connector definition, all logic entries (persisted and temporary), and any source instances that reference it.

### Dropping Connector Logic

To remove connector logic from a connector, use `drop_connector_logic()` (persisted) or `drop_temporary_connector_logic()` (runtime-only).

=== "Async API"

    ```python
    # Drop all logic entries (persisted)
    bundle = await bundle.drop_connector_logic('example.connector')

    # Drop logic for a specific platform
    bundle = await bundle.drop_connector_logic('example.connector', platform='linux/amd64')

    # Drop temporary (runtime-only) logic
    count = await bundle.drop_temporary_connector_logic('example.connector')
    ```

=== "Sync API"

    ```python
    # Drop all logic entries (persisted)
    bundle.drop_connector_logic('example.connector')

    # Drop logic for a specific platform
    bundle.drop_connector_logic('example.connector', platform='linux/amd64')

    # Drop temporary (runtime-only) logic
    count = bundle.drop_temporary_connector_logic('example.connector')
    ```

=== "SQL"

    ```sql
    -- Drop all logic entries (persisted)
    DROP CONNECTOR LOGIC example.connector

    -- Drop logic for a specific platform
    DROP CONNECTOR LOGIC example.connector FOR PLATFORM 'linux/amd64'

    -- Drop temporary (runtime-only) logic
    DROP TEMPORARY CONNECTOR LOGIC example.connector
    DROP TEMPORARY CONNECTOR LOGIC example.connector FOR PLATFORM 'linux/amd64'
    ```

## Fetching Data

### fetch()

Discovers and attaches new files from a specific pack's sources. Returns a list of `FetchResults`, one for each source.

=== "Async API"

    ```python
    # Fetch from base pack (default)
    results = await bundle.fetch("base", "add")
    for result in results:
        print(f"{result.source_function}: {len(result.added)} added")

    # Fetch from a joined pack
    results = await bundle.fetch("customers", "add")
    ```

=== "Sync API"

    ```python
    # Fetch from base pack (default)
    results = bundle.fetch("base", "add")
    for result in results:
        print(f"{result.source_function}: {len(result.added)} added")

    # Fetch from a joined pack
    results = bundle.fetch("customers", "add")
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
    results = await bundle.fetch_all("add")
    for result in results:
        print(f"{result.pack}/{result.source_function}: {result.total_count()} changes")
    ```

=== "Sync API"

    ```python
    results = bundle.fetch_all("add")
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
    results = await bundle.fetch("customers", "add")
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
    results = bundle.fetch("customers", "add")
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
    results = await bundle.fetch("base", "add")
    total_added = sum(len(r.added) for r in results)
    print(f"Initial load: {total_added} files")
    await bundle.commit("Initial data load")

    # ... time passes, new files appear in S3 ...

    # Incremental load (only attaches new files)
    bundle = (await bb.open("sales/data")).extend()
    results = await bundle.fetch("base", "add")
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
    results = bundle.fetch("base", "add")
    total_added = sum(len(r.added) for r in results)
    print(f"Initial load: {total_added} files")
    bundle.commit("Initial data load")

    # ... time passes, new files appear in S3 ...

    # Incremental load (only attaches new files)
    bundle = bb.open("sales/data").extend()
    results = bundle.fetch("base", "add")
    total_added = sum(len(r.added) for r in results)
    if total_added > 0:
        print(f"Loaded {total_added} new files")
        bundle.commit("Incremental data load")
    ```
