# Sources

Sources allow you to define where data files come from and automatically discover and attach new files as they become available. This is useful for working with directories of files that grow over time, such as daily data exports or streaming data partitions.

## Overview

The source workflow has two steps:

1. **Define a source** with `CREATE SOURCE` - Specifies where to look for files
2. **Fetch new files** with `FETCH` - Discovers and attaches any new files found

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
    CREATE SOURCE USING remote_dir WITH (url = 's3://my-bucket/data/', patterns = '**/*.parquet')
    ```

## Connectors

Connectors define how to pull data from an external source into your bundle.

Bundlebase ships with several common connectors, but custom connectors can also be made and plugged in to support any external data you need.

### http

Downloads data from a single HTTP(S) URL. Use this for any direct link to a CSV, JSON, or Parquet file — including REST API endpoints that return data.

**Arguments:**

| Argument | Required | Description |
|----------|----------|-------------|
| `url` | Yes | The HTTP(S) URL to download |
| `format` | No | Force file format (`csv`, `json`, `parquet`). Auto-detected from URL extension if omitted |

=== "SQL"

    ```sql
    -- Download a CSV from a URL
    CREATE SOURCE USING http WITH (url = 'https://data.mn.gov/api/lake_quality.csv')

    -- Force format when URL has no extension
    CREATE SOURCE USING http WITH (url = 'https://api.example.com/data?format=csv', format = 'csv')
    ```

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
    CREATE SOURCE USING remote_dir WITH (url = 's3://my-bucket/data/', patterns = '**/*.parquet')
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
    CREATE SOURCE USING ftp_directory WITH (url = 'ftp://ftp.example.com/pub/data/')
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
    CREATE SOURCE USING sftp_directory WITH (url = 'sftp://user@host/data/', key_path = '~/.ssh/id_rsa', patterns = '**/*.parquet')
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
    CREATE SOURCE USING kaggle WITH (dataset = 'zillow/zecon', patterns = '*.csv')
    ```

### Custom Connectors

Bundlebase supports custom connectors in two modes — **native** (in-process, zero-copy) and **IPC** (subprocess). Custom connectors use a two-step workflow: [`IMPORT CONNECTOR`](custom-connectors/index.md#import-connector) or [`IMPORT TEMP CONNECTOR`](custom-connectors/index.md#import-temp-connector), then [`CREATE SOURCE`](custom-connectors/index.md#create-source).

#### native (In-Process, Zero-Copy)

```python
bundle.import_temp_connector('example.connector', 'python::example_connector:ExampleConnector')
bundle.create_source('example.connector')
```

#### IPC (Subprocess)

```python
bundle.import_connector('example.connector', 'ipc::./example_connector')
bundle.create_source('example.connector')
```

See [Custom Connectors](custom-connectors/index.md) for full command reference, runtime values, SDKs, and protocol details.

### Dropping Connectors

To remove custom connectors, see the full command reference:

- [`DROP CONNECTOR`](custom-connectors/index.md#drop-connector) — removes the connector (or just a specific platform's entry)
- [`DROP TEMP CONNECTOR`](custom-connectors/index.md#drop-temp-connector) — removes runtime-only connector entries

## Fetching Data

### fetch()

Discovers and attaches new files from a specific pack's sources. Returns a list of `FetchResults`, one for each source.

=== "Async API"

    ```python
    # Fetch from base pack (default)
    results = await bundle.fetch("base", "add")
    for result in results:
        print(f"{result.connector}: {len(result.added)} added")

    # Fetch from a joined pack
    results = await bundle.fetch("customers", "add")
    ```

=== "Sync API"

    ```python
    # Fetch from base pack (default)
    results = bundle.fetch("base", "add")
    for result in results:
        print(f"{result.connector}: {len(result.added)} added")

    # Fetch from a joined pack
    results = bundle.fetch("customers", "add")
    ```

=== "SQL"

    ```sql
    FETCH base ADD

    FETCH customers ADD
    ```

#### Dry Run

Use `DRY RUN` (SQL) to preview what a fetch would do without actually attaching or removing any files. The returned `FetchResults` show what would be added, replaced, or removed.

=== "SQL"

    ```sql
    FETCH base ADD DRY RUN

    FETCH ALL SYNC DRY RUN
    ```

### fetch_all()

Discovers and attaches new files from all defined sources across all packs. Returns a list of `FetchResults`, one for each source (including sources with no changes).

=== "Async API"

    ```python
    results = await bundle.fetch_all("add")
    for result in results:
        print(f"{result.pack}/{result.connector}: {result.total_count()} changes")
    ```

=== "Sync API"

    ```python
    results = bundle.fetch_all("add")
    for result in results:
        print(f"{result.pack}/{result.connector}: {result.total_count()} changes")
    ```

=== "SQL"

    ```sql
    FETCH ALL ADD
    ```

### FetchResults

Each `FetchResults` object contains:

| Property | Type | Description |
|----------|------|-------------|
| `connector` | `str` | Connector name (e.g., "remote_dir") |
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
    bundle = await bundle.join("customers", "bundle.customer_id = customers.id")

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
    bundle = bundle.join("customers", "bundle.customer_id = customers.id")

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
    CREATE SOURCE FOR customers USING remote_dir WITH (url = 's3://bucket/customers/', patterns = '**/*.parquet')
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
