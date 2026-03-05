# Custom Source Functions

Custom source functions let you write data providers in any language. The `type_` parameter determines how Bundlebase loads and communicates with your source:

| Type | How It Works | Performance | Languages |
|------|-------------|-------------|-----------|
| **`python`** | In-process via PyO3 | Zero-copy Arrow | Python |
| **`lib`** | In-process via `dlopen` of a shared library | Zero-copy Arrow | Rust, Go, Java |
| **`java`** | Subprocess via `java -jar` | Serialized Arrow IPC | Java |
| **`docker`** | Subprocess via `docker run` | Serialized Arrow IPC | Any language |
| **`ipc`** | Subprocess via direct command execution | Serialized Arrow IPC | Any language |

Internally, `python` and `lib` run **in-process** (native mode) for zero-copy Arrow transfer. `java`, `docker`, and `ipc` run as **subprocesses** communicating over stdin/stdout.

**Your source code is the same regardless of type** — only the entry point differs. SDKs for Python, Go, Java, and Rust handle the protocol automatically.

## Choosing a Type

**Use `python` when:**

- Your source is a Python class in the same project
- You need maximum performance with zero serialization overhead
- Note: requires `set_temporary_connector_logic()` since Python code can't be bundled

**Use `lib` when:**

- You have a compiled shared library (`.so`/`.dylib`/`.dll`) from Rust, Go, or Java
- You need zero-copy performance with a portable, bundled source

**Use `java`, `docker`, or `ipc` when:**

- You want process isolation (source crashes don't affect Bundlebase)
- You're packaging your source as a Docker image (`docker`)
- You're running a Java JAR (`java`)
- You're running any other executable (`ipc`)

## Configuration

!!! warning "External Code Execution"
    Custom source functions that execute external code (Python native sources, shared libraries, IPC subprocesses) require the `allow_external_code` configuration setting:

    ```python
    config = {"system": {"allow_external_code": "true"}}
    bundle = bb.create("my/data", config=config)
    ```

    Without this, `create_source()` will fail with `"External code execution is disabled"`.

## How It Works

### Native Mode

**Python:** Source objects are called directly in-process via PyO3 — no subprocess, no serialization.

**Compiled languages:** Build a shared library (`.so`/`.dylib`/`.dll`) exporting the [C ABI](native.md#c-abi-reference). Bundlebase `dlopen`s it and uses the Arrow C Data Interface.

### IPC Mode

A custom source function runs as a subprocess that Bundlebase launches and communicates with over stdin/stdout:

1. **Discover** — Bundlebase sends a `discover` call. Your source returns a list of available data locations.
2. **Data** — For each location, Bundlebase sends a `data` call. Your source returns Arrow record batches.
3. **Stable URL** (optional) — Bundlebase may send a `stable_url` call to check if a location has a cached URL.
4. **Shutdown** — Bundlebase sends a `shutdown` call and the subprocess exits.

## Key Concepts

### Location

A `Location` represents a discovered data file. Every SDK provides this type with the same fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `location` | string | *(required)* | Identifier for the data file (e.g., `"data/file1.parquet"`) |
| `must_copy` | bool | `true` | Whether the data must be copied into the bundle |
| `format` | string | `"parquet"` | File format hint |
| `version` | string | `""` | Version string for change detection |

### StableUrl

A `StableUrl` contains a single `url` field. When provided, Bundlebase can cache the data at that URL and skip re-fetching on subsequent runs if the URL hasn't changed.

### Extra Arguments

Any extra key-value arguments passed in the source configuration are forwarded to your `discover`, `data`, and `stable_url` methods. This lets you parameterize your source without changing code.

## Using a Custom Source

Custom sources use a three-step API: `create_connector()` declares the connector name, then either `set_connector_logic()` (persisted) or `set_temporary_connector_logic()` (runtime-only) configures how it runs, and `create_source()` activates it with any extra arguments.

- **`set_connector_logic()`** — Persists the logic into the bundle. Use for portable bundles. Rejects `type_='python'` since Python code can't be bundled.
- **`set_temporary_connector_logic()`** — Sets logic at runtime only. Use for `type_='python'` in-process sources. Works on both `Bundle` and `BundleBuilder`.
- **`drop_connector()`** — Removes the connector definition and all associated logic and sources.
- **`drop_connector_logic()`** — Removes persisted logic. Optionally filter by platform with `platform='linux/amd64'`.
- **`drop_temporary_connector_logic()`** — Removes runtime-only logic. Works on both `Bundle` and `BundleBuilder`.

### Native Mode (Recommended for Python)

```python
import bundlebase.sync as bb

# Python native — zero-copy, in-process (runtime-only)
bundle = bb.create("my/data")
bundle.create_connector('example.connector')
bundle.set_temporary_connector_logic('example.connector', type_='python', logic='example_connector:ExampleConnector')
bundle.create_source('example.connector')
results = bundle.fetch("base", "add")
```

### Native Mode (Shared Library)

```python
# Rust, Go, or Java — zero-copy via dlopen (persisted into bundle)
bundle.create_connector('example.connector')
bundle.set_connector_logic('example.connector', type_='lib', logic='./target/release/libexample_connector.so')
bundle.create_source('example.connector')
```

### IPC Mode (Subprocess)

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await bb.create("my/data")
    bundle = await bundle.create_connector('example.connector')
    bundle = await bundle.set_connector_logic('example.connector', type_='ipc', logic='./example_connector')
    bundle = await bundle.create_source('example.connector')

    results = await bundle.fetch("base", "add")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = bb.create("my/data")
    bundle.create_connector('example.connector')
    bundle.set_connector_logic('example.connector', type_='ipc', logic='./example_connector')
    bundle.create_source('example.connector')

    results = bundle.fetch("base", "add")
    ```

=== "SQL"

    ```sql
    CREATE CONNECTOR example.connector
    SET CONNECTOR LOGIC example.connector WITH (type = 'ipc', logic = './example_connector')
    CREATE SOURCE example.connector
    ```

## Type Values

The `type_` parameter determines how Bundlebase loads and runs the source:

| Type | Mode | `logic` value | What happens |
|------|------|--------------|--------------|
| `python` | Native (in-process) | `module:Class` | Imports the Python class via PyO3 and calls it directly |
| `lib` | Native (in-process) | Path to `.so`/`.dylib`/`.dll` | `dlopen`s the shared library and uses Arrow C Data Interface |
| `java` | IPC (subprocess) | Path to JAR file | Runs `java -jar <logic>` as a subprocess |
| `docker` | IPC (subprocess) | Docker image name | Runs `docker run -i --rm <logic>` as a subprocess |
| `ipc` | IPC (subprocess) | Command to run | Executes `<logic>` directly (whitespace-split) as a subprocess |

!!! note
    `set_connector_logic()` rejects `type_='python'` because Python code cannot be bundled. Use `set_temporary_connector_logic()` for Python sources.

## Docker Sources

Package any source as a Docker image:

```dockerfile
FROM python:3.13-slim
RUN pip install bundlebase-sdk pyarrow
COPY example_connector.py /app/example_connector.py
CMD ["python", "/app/example_connector.py"]
```

Use with:

```python
bundle.create_connector('example.connector')
bundle.set_connector_logic('example.connector', type_='docker', logic='myorg/example-connector:latest')
bundle.create_source('example.connector')
```

The container receives JSON-RPC on stdin and writes responses to stdout.

## SDK Quick Start

Each SDK handles the protocol for you. Implement the source interface and choose your entry point — `serve()` for IPC mode or the native export for zero-copy mode.

=== "Python"

    ```python
    from bundlebase_sdk import SourceFunction, Location, serve
    import pyarrow as pa

    class ExampleConnector(SourceFunction):
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
    public class ExampleConnector implements SourceFunction {
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

    impl SourceFunction for ExampleConnector {
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

For implementing sources in languages without an SDK.

**Transport**: Line-delimited JSON-RPC 2.0 on stdin/stdout.

### Methods

**`discover`** — Returns available locations.

Request params: `{"attached_locations": ["loc1", ...], ...extra_args}`

Response: `{"locations": [{"location": "...", "must_copy": true, "format": "parquet", "version": "v1"}, ...]}`

**`data`** — Returns data for a location.

Request params: `{"location": {"location": "...", "must_copy": true, "format": "...", "version": "..."}, ...extra_args}`

Response: `{"ok": true}` followed by a length-prefixed Arrow IPC frame.

**`stable_url`** — Returns a stable URL (optional).

Response: `{"url": "https://..."}` or `null`.

**`shutdown`** — Clean exit.

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
