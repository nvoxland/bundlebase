# Custom Connectors

Custom connectors let you write data providers in any language. The `runtime` parameter determines how Bundlebase loads and communicates with your connector:

| Type | How It Works | Performance | Languages |
|------|-------------|-------------|-----------|
| **`python`** | In-process via PyO3 | Zero-copy Arrow | Python |
| **`ffi`** | In-process via `dlopen` of a shared library | Zero-copy Arrow | Rust, Go, Java |
| **`java`** | Subprocess via `java -jar` | Serialized Arrow IPC | Java |
| **`docker`** | Subprocess via `docker run` | Serialized Arrow IPC | Any language |
| **`ipc`** | Subprocess via direct command execution | Serialized Arrow IPC | Any language |

Internally, `python` and `ffi` run **in-process** (native mode) for zero-copy Arrow transfer. `java`, `docker`, and `ipc` run as **subprocesses** communicating over stdin/stdout.

**Your source code is the same regardless of type**; only the entry point differs. SDKs for Python, Go, Java, and Rust handle the protocol automatically.

## Runtime URI Format Reference

When importing a connector, the `FROM` clause uses a `runtime::entrypoint` URI. This table shows the format for each runtime:

| Runtime | URI Format | Example |
|---------|-----------|---------|
| `ipc` | `ipc::command` | `ipc::python my_module.py` |
| `ffi` | `ffi::path[:symbol]` | `ffi::libexample.so:my_func` |
| `python` | `python::module:Class` | `python::my_source:MyConnector` |
| `java` | `java::path.jar` | `java::connectors/my.jar` |
| `docker` | `docker::image:tag` | `docker::myorg/myconnector:latest` |

## Built-in Connectors

Bundlebase includes several built-in connectors that don't need to be imported:

| Connector | Description |
|-----------|-------------|
| `remote_dir` | Files from a remote directory (S3, HTTP, etc.) |
| `ftp_directory` | Files from an FTP server |
| `sftp_directory` | Files from an SFTP server |
| `kaggle` | Datasets from Kaggle |
| `web_scrape` | Data scraped from web pages |
| `postgres` | Data from a PostgreSQL database |

Use these directly with `CREATE SOURCE`; no `IMPORT CONNECTOR` step needed.

## Overview

Custom connectors use a simple workflow:

1. [**Load the connector**](#import-connector) -- defines a named connector
2. [**Create a source**](#create-source) -- creates a source instance from the connector
3. [**Fetch data**](#fetch) -- discovers and attaches data from the source

To remove or rename connectors:

- [**Drop connector**](#drop-connector) -- removes the connector (or just a specific platform's entry)
- [**Drop temp connector**](#drop-temp-connector) -- removes runtime-only connector entries
- [**Rename connector**](#rename-connector) -- renames a connector and updates associated sources
- [**Rename temp connector**](#rename-temp-connector) -- renames runtime-only connector entries

## Choosing a Runtime

**Use `python` when:**

- Your source is a Python class in the same project
- You need maximum performance with zero serialization overhead
- Note: requires [`IMPORT TEMP CONNECTOR`](#import-temp-connector) since Python code can't be bundled

**Use `ffi` when:**

- You have a compiled shared library (`.so`/`.dylib`/`.dll`) from Rust, Go, or Java
- You need zero-copy performance with a portable, bundled source

**Use `java`, `docker`, or `ipc` when:**

- You want process isolation (source crashes don't affect Bundlebase)
- You're packaging your connector as a Docker image (`docker`)
- You're running a Java JAR (`java`)
- You're running any other executable (`ipc`)

## Configuration

!!! warning "External Code Execution"
    Custom connectors that execute external code (Python native connectors, shared libraries, IPC subprocesses) require the `allow_external_code` configuration setting:

    ```python
    config = {"system": {"allow_external_code": "true"}}
    bundle = bb.create("my/data", config=config)
    ```

    Without this, [`CREATE SOURCE`](#create-source) will fail with `"External code execution is disabled"`.

## Commands

### IMPORT CONNECTOR

Creates a named connector. The connector definition is **persisted** into the bundle's commit history, making the bundle portable.

=== "Async API"

    ```python
    bundle = await bundle.import_connector(
        'acme.weather',
        'ipc::my_connector'
    )
    ```

=== "Sync API"

    ```python
    bundle.import_connector(
        'acme.weather',
        'ipc::my_connector'
    )
    ```

=== "SQL"

    ```sql
    IMPORT CONNECTOR acme.weather FROM 'ipc::my_connector'
    ```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | `str` | *(required)* | Dot-separated connector name (e.g., `"acme.weather"`) |
| `from_` | `str` | *(required)* | Runtime URI in `runtime::entrypoint` format (e.g., `"ipc::my_connector"`, `"python::my_module:MyConnector"`) |
| `platform` | `str` | `"*/*"` | Target platform (e.g., `"linux/amd64"`, `"darwin/arm64"`, `"*/*"` for all) |

!!! note
    `IMPORT CONNECTOR` rejects `runtime='python'` because Python code cannot be serialized into the bundle. Use [`IMPORT TEMP CONNECTOR`](#import-temp-connector) for Python connectors.

Connector names use a **dot-separated namespace** format. The part before the first dot is the **namespace** (e.g., `acme` in `acme.weather`). Choose a namespace that is unique to you or your organization to prevent naming collisions when sharing bundles. For example:

- `mycompany.sales.crm` -- "mycompany" namespace
- `jdoe.weather.noaa` -- personal namespace
- `acme.weather` -- organization namespace

The name must contain exactly one dot.

You can call `IMPORT CONNECTOR` multiple times for different platforms on the same connector; the last call for a given platform wins. At runtime, Bundlebase selects the best match for the current OS/architecture.

#### Multi-platform in one statement

Instead of calling `IMPORT CONNECTOR` once per target, the SQL form accepts a **platform map** or a **glob** with `{os}`, `{arch}`, `{ext}` placeholders. Both desugar to N entries (one per platform) committed atomically.

```sql
-- Map form: list every platform explicitly
IMPORT CONNECTOR acme.weather FROM {
    'linux/amd64'   : 'ffi::./weather-linux-amd64.so',
    'linux/arm64'   : 'ffi::./weather-linux-arm64.so',
    'darwin/arm64'  : 'ffi::./weather-darwin-arm64.dylib',
    'windows/amd64' : 'ffi::./weather-windows-amd64.dll'
};

-- Glob form: scan a directory for matching files
IMPORT CONNECTOR acme.weather FROM 'ffi::./weather-{os}-{arch}.{ext}';
```

Foreign-platform binaries are checked structurally (ELF / Mach-O / PE magic + arch byte) since the build host can't `dlopen` them. The host-platform binary still gets full `dlopen` verification.

#### Bundling Connector Source

Add `WITH (src = '...')` to ship the connector's source code (e.g. a release zip) inside the bundle:

```sql
IMPORT CONNECTOR acme.weather FROM 'ffi::./lib.so'
    WITH (platform = 'linux/amd64', src = './weather-source.zip');
```

The archive is copied into the bundle's content-addressed data directory and travels with empty exports. Recipients can extract it with [`EXPORT SOURCE`](../../sql-reference/index.md#export-source). Multi-platform `IMPORT CONNECTOR` shares a single source archive across every platform entry.

---

### IMPORT TEMP CONNECTOR

Creates a connector at **runtime only**; nothing is persisted into the bundle. Use this for `runtime='python'` in-process connectors. Works on both `Bundle` (read-only) and `BundleBuilder`.

=== "Async API"

    ```python
    bundle = await bundle.import_temp_connector(
        'acme.weather',
        'python::my_module:MyConnector'
    )
    ```

=== "Sync API"

    ```python
    bundle.import_temp_connector(
        'acme.weather',
        'python::my_module:MyConnector'
    )
    ```

=== "SQL"

    ```sql
    IMPORT TEMP CONNECTOR acme.weather FROM 'python::my_module:MyConnector'
    ```

**Parameters:** Same as [`IMPORT CONNECTOR`](#import-connector), but the `from_` parameter accepts all runtimes including `python`.

Temporary connectors take precedence over persisted connectors when both exist for the same platform. This is useful for development workflows where you want to test a Python connector locally before packaging it as a shared library or Docker image.

---

### CREATE SOURCE

Creates a source instance from a connector. For built-in connectors (`remote_dir`, `kaggle`, etc.), this is a single step. For custom connectors, you must first [`IMPORT CONNECTOR`](#import-connector) or [`IMPORT TEMP CONNECTOR`](#import-temp-connector).

=== "Async API"

    ```python
    # Custom connector (no extra args)
    bundle = await bundle.create_source('acme.weather')

    # Custom connector with extra args forwarded to discover/data
    bundle = await bundle.create_source('acme.weather', {
        'region': 'us-east'
    })

    # Built-in connector
    bundle = await bundle.create_source('remote_dir', {
        'url': 's3://bucket/data/',
        'patterns': '**/*.parquet'
    })

    # Source on a specific pack
    bundle = await bundle.create_source('remote_dir', {
        'url': 's3://bucket/customers/'
    }, pack='customers')
    ```

=== "Sync API"

    ```python
    # Custom connector (no extra args)
    bundle.create_source('acme.weather')

    # Custom connector with extra args forwarded to discover/data
    bundle.create_source('acme.weather', {
        'region': 'us-east'
    })

    # Built-in connector
    bundle.create_source('remote_dir', {
        'url': 's3://bucket/data/',
        'patterns': '**/*.parquet'
    })

    # Source on a specific pack
    bundle.create_source('remote_dir', {
        'url': 's3://bucket/customers/'
    }, pack='customers')
    ```

=== "SQL"

    ```sql
    -- Custom connector (no extra args)
    CREATE SOURCE USING acme.weather

    -- Custom connector with extra args
    CREATE SOURCE USING acme.weather WITH (region = 'us-east')

    -- Built-in connector
    CREATE SOURCE USING remote_dir WITH (url = 's3://bucket/data/', patterns = '**/*.parquet')

    -- Source on a specific pack
    CREATE SOURCE FOR customers USING remote_dir WITH (url = 's3://bucket/customers/')
    ```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `connector` | `str` | *(required)* | Connector name -- either a built-in name or a dot-separated custom connector name |
| `args` | `dict` | `{}` | Key-value arguments passed to the connector. For custom connectors, these are forwarded as extra arguments to `discover()`, `data()`, and `stable_url()` |
| `pack` | `str` | `"base"` | Which pack to attach discovered files to |

---

### FETCH

Discovers and attaches new files from sources. Returns a list of `FetchResults`, one per source. See [Data Sources](../sources.md#fetching-data) for full details on fetch modes and results.

=== "Async API"

    ```python
    # Fetch from a specific pack
    results = await bundle.fetch("base", "add")

    # Fetch from all packs
    results = await bundle.fetch_all("add")
    ```

=== "Sync API"

    ```python
    # Fetch from a specific pack
    results = bundle.fetch("base", "add")

    # Fetch from all packs
    results = bundle.fetch_all("add")
    ```

=== "SQL"

    ```sql
    -- Fetch from a specific pack
    FETCH base ADD

    -- Fetch from all packs
    FETCH ALL ADD
    ```

**Fetch modes:** `add` (only add new files), `update` (add new + update changed), `sync` (add + update + remove missing).

---

### DROP CONNECTOR

Removes a connector. Without a platform, removes the entire connector definition and **all** its entries (persisted and temporary) and source instances. With a platform, removes only the connector for that platform.

Like other drop operations, the bundled artifacts are not deleted; a `DropConnectorOp` is added to the history.

=== "Async API"

    ```python
    # Drop the entire connector
    bundle = await bundle.drop_connector('acme.weather')

    # Drop connector for a specific platform only
    bundle = await bundle.drop_connector('acme.weather', platform='linux/amd64')
    ```

=== "Sync API"

    ```python
    # Drop the entire connector
    bundle.drop_connector('acme.weather')

    # Drop connector for a specific platform only
    bundle.drop_connector('acme.weather', platform='linux/amd64')
    ```

=== "SQL"

    ```sql
    -- Drop the entire connector
    DROP CONNECTOR acme.weather

    -- Drop connector for a specific platform only
    DROP CONNECTOR acme.weather FOR PLATFORM 'linux/amd64'
    ```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | `str` | *(required)* | Dot-separated connector name |
| `platform` | `str` | `None` | If specified, only drop the connector for this platform. If `None`, drops the entire connector. |

---

### DROP TEMP CONNECTOR

Removes **runtime-only** connector entries. Works on both `Bundle` and `BundleBuilder`.

=== "Async API"

    ```python
    # Drop all temporary connectors
    count = await bundle.drop_temp_connector('acme.weather')

    # Drop temporary connector for a specific platform
    count = await bundle.drop_temp_connector('acme.weather', platform='*/*')
    ```

=== "Sync API"

    ```python
    # Drop all temporary connectors
    count = bundle.drop_temp_connector('acme.weather')

    # Drop temporary connector for a specific platform
    count = bundle.drop_temp_connector('acme.weather', platform='*/*')
    ```

=== "SQL"

    ```sql
    -- Drop all temporary connectors
    DROP TEMP CONNECTOR acme.weather

    -- Drop temporary connector for a specific platform
    DROP TEMP CONNECTOR acme.weather FOR PLATFORM '*/*'
    ```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | `str` | *(required)* | Dot-separated connector name |
| `platform` | `str` | `None` | If specified, only drop the connector for this platform. If `None`, drops all platforms. |

**Returns:** The number of connector entries removed.

---

### RENAME CONNECTOR

Renames a connector definition to a new dotted name. All platform entries are renamed, and any sources referencing the old connector name are automatically updated.

=== "Async API"

    ```python
    bundle = await bundle.rename_connector('acme.weather', 'acme.weather_v2')
    ```

=== "Sync API"

    ```python
    bundle.rename_connector('acme.weather', 'acme.weather_v2')
    ```

=== "SQL"

    ```sql
    RENAME CONNECTOR acme.weather TO acme.weather_v2
    ```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `old_name` | `str` | *(required)* | Current dot-separated connector name |
| `new_name` | `str` | *(required)* | New dot-separated connector name |

---

### RENAME TEMP CONNECTOR

Renames **runtime-only** connector entries to a new name. Only temporary entries are renamed; persistent entries are not affected.

=== "SQL"

    ```sql
    RENAME TEMP CONNECTOR acme.weather TO acme.weather_v2
    ```

**Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `old_name` | `str` | *(required)* | Current dot-separated connector name |
| `new_name` | `str` | *(required)* | New dot-separated connector name |

## How It Works

### Native Mode

**Python:** Source objects are called directly in-process via PyO3; no subprocess, no serialization.

**Compiled languages:** Build a shared library (`.so`/`.dylib`/`.dll`) exporting the [C ABI](native.md#c-abi-reference). Bundlebase `dlopen`s it and uses the Arrow C Data Interface.

### IPC Mode

An IPC connector runs as a subprocess that Bundlebase launches and communicates with over stdin/stdout:

1. **Discover** -- Bundlebase sends a `discover` call. Your source returns a list of available data locations.
2. **Data** -- For each location, Bundlebase sends a `data` call. Your source returns Arrow record batches.
3. **Stable URL** (optional) -- Bundlebase may send a `stable_url` call to check if a location has a cached URL.
4. **Shutdown** -- Bundlebase sends a `shutdown` call and the subprocess exits.

## Key Concepts

### Location

A `Location` represents a discovered data file. Every SDK provides this type with the same fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `location` | string | *(required)* | Identifier for the data file (e.g., `"data/file1.parquet"`) |
| `must_copy` | bool | `true` | Whether the data must be copied into the bundle |
| `format` | string | `"parquet"` | File format hint |
| `version` | string | `""` | Version string for change detection |
| `num_rows` | int \| null | `null` | Optional row count used by `FETCH ... DRY RUN` to report expected row deltas without reading the data. Set when the connector can determine it cheaply (Parquet footer, manifest); set to `null` when counting would require fully parsing the data. `null` is preserved distinct from `0` in `FETCH` output. |

### StableUrl

A `StableUrl` contains a single `url` field. When provided, Bundlebase can cache the data at that URL and skip re-fetching on subsequent runs if the URL hasn't changed.

### Extra Arguments

Any extra key-value arguments passed in the [`CREATE SOURCE`](#create-source) configuration are forwarded to your `discover`, `data`, and `stable_url` methods. This lets you parameterize your connector without changing code.

## Runtime Values

The `runtime` parameter determines how Bundlebase loads and runs the connector:

| Runtime  | Mode | `entrypoint` value | What happens |
|----------|------|--------------|--------------|
| `python` | Native (in-process) | `module:Class` | Imports the Python class via PyO3 and calls it directly |
| `ffi`    | Native (in-process) | Path to `.so`/`.dylib`/`.dll` | `dlopen`s the shared library and uses Arrow C Data Interface |
| `java`   | IPC (subprocess) | Path to JAR file | Runs `java -jar <entrypoint>` as a subprocess |
| `docker` | IPC (subprocess) | Docker image name | Runs `docker run -i --rm <entrypoint>` as a subprocess |
| `ipc`    | IPC (subprocess) | Command to run | Executes `<entrypoint>` directly (whitespace-split) as a subprocess |

## Docker Connectors

Package any connector as a Docker image:

```dockerfile
FROM python:3.13-slim
RUN pip install bundlebase-sdk pyarrow
COPY example_connector.py /app/example_connector.py
CMD ["python", "/app/example_connector.py"]
```

Use with [`IMPORT CONNECTOR`](#import-connector):

```python
bundle.import_connector('example.connector', 'docker::myorg/example-connector:latest')
bundle.create_source('example.connector')
```

The container receives JSON-RPC on stdin and writes responses to stdout.

## Getting Started

!!! tip "Scaffold a new project"
    Use the `bundlebase init-sdk` command to generate a new connector or function project with all the boilerplate:

    ```bash
    # Scaffold a Python connector project
    bundlebase init-sdk python my_connector --type connector

    # Scaffold a Go function project
    bundlebase init-sdk go my_functions --type function

    # Scaffold both connector and function
    bundlebase init-sdk rust my_project --type both
    ```

    Supported languages: `python`, `go`, `java`, `rust`.

## SDK Quick Start

Each SDK handles the protocol for you. Implement the connector interface and choose your entry point: `serve()` for IPC mode or the native export for zero-copy mode.

=== "Python"

    ```python
    from bundlebase_sdk import Connector, Location, serve
    import pyarrow as pa

    class ExampleConnector(Connector):
        def discover(self, attached_locations, **kwargs):
            return [Location("data.parquet", format="parquet", version="v1")]

        def data(self, location, **kwargs):
            return pa.table({"id": [1, 2, 3], "value": ["a", "b", "c"]})

    if __name__ == "__main__":
        serve(ExampleConnector())
    ```

    See the [Python SDK](python.md) reference for full API details.

=== "Go"

    ```go
    type ExampleConnector struct{}

    func (s *ExampleConnector) Discover(attached []string, args map[string]string) ([]sdk.Location, error) {
        return []sdk.Location{
            {Location: "data.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
        }, nil
    }

    func (s *ExampleConnector) Data(loc sdk.Location, args map[string]string) ([]arrow.Record, error) {
        // Build and return Arrow records
    }

    func main() { sdk.Serve(&ExampleConnector{}) }
    ```

    See the [Go SDK](go.md) reference for full API details.

=== "Java"

    ```java
    public class ExampleConnector implements Connector {
        public List<Location> discover(List<String> attached, Map<String, String> args) {
            return List.of(new Location("data.parquet", true, "parquet", "v1"));
        }

        public VectorSchemaRoot data(Location loc, Map<String, String> args) {
            // Build and return Arrow VectorSchemaRoot
        }

        public static void main(String[] args) { Serve.run(new ExampleConnector()); }
    }
    ```

    See the [Java SDK](java.md) reference for full API details.

=== "Rust"

    ```rust
    struct ExampleConnector;

    impl Connector for ExampleConnector {
        fn discover(&self, _attached: &[String], _args: &HashMap<String, String>)
            -> Result<Vec<Location>, Box<dyn std::error::Error>> {
            Ok(vec![Location { location: "data.parquet".into(), ..Location::new("data.parquet") }])
        }

        fn data(&self, _location: &Location, _args: &HashMap<String, String>)
            -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
            // Build and return Arrow RecordBatches
        }
    }

    fn main() { bundlebase_sdk::serve(&ExampleConnector); }
    ```

    See the [Rust SDK](rust.md) reference for full API details.

## Protocol Reference

For implementing connectors in languages without an SDK.

**Transport**: Line-delimited JSON-RPC 2.0 on stdin/stdout.

### Health Check

**`ping`** -- Returns `"pong"`. Used by Bundlebase to verify the subprocess is alive and responsive. All SDKs handle this automatically.

### Reserved Argument Keys

The `args` map passed to `discover`, `data`, and `stable_url` may contain reserved keys prefixed with `_`:

- **`_columns`** -- A comma-separated list of column names that the caller wants. Connectors that support column pushdown can parse this key to return only the requested columns, reducing data transfer. It is safe to ignore this key.

### Methods

**`discover`** -- Returns available locations.

Request params: `{"attached_locations": ["loc1", ...], ...extra_args}`

Response: `{"locations": [{"location": "...", "must_copy": true, "format": "parquet", "version": "v1", "num_rows": 1234}, ...]}`

`num_rows` must be present (integer or JSON `null`); see the [Location](#location) field reference.

**`data`** -- Returns data for a location.

Request params: `{"location": {"location": "...", "must_copy": true, "format": "...", "version": "...", "num_rows": null}, ...extra_args}`

Response: `{"ok": true}` followed by a length-prefixed Arrow IPC frame.

**`stable_url`** -- Returns a stable URL (optional).

Response: `{"url": "https://..."}` or `null`.

**`shutdown`** -- Clean exit.

Response: `{"ok": true}`, then exit.

### Arrow IPC Framing

After the `data` JSON response line, write:

1. **4 bytes**: Big-endian `u32` length of the IPC data
2. **N bytes**: Arrow IPC stream bytes

Write a zero-length prefix (`\x00\x00\x00\x00`) for no data.

### Error Handling

Return JSON-RPC errors for failures:

```json
{"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "description"}}
```

Standard codes: `-32601` (method not found), `-32000` (application error).
