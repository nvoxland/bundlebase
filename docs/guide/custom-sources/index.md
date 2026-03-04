# Custom Source Functions

Custom source functions let you write data providers in any language. Bundlebase supports two modes for loading custom sources:

| Mode | How It Works | Performance | Languages |
|------|-------------|-------------|-----------|
| **[Plugin](plugin.md)** | In-process loading (Python PyO3 or shared library `dlopen`) | Zero-copy Arrow | Python, Rust, Go, Java |
| **IPC** | Subprocess with JSON-RPC over stdin/stdout | Serialized Arrow IPC | Any language |

**Your source code is the same for both modes** — only the entry point differs. SDKs for Python, Go, Java, and Rust handle the protocol automatically.

## Choosing IPC vs Plugin

**Use plugin when:**

- You need maximum performance (zero-copy, no serialization overhead)
- Your source is in Python, Rust, Go, or Java
- Your source is part of the same project

**Use IPC when:**

- You want process isolation (source crashes don't affect Bundlebase)
- You're packaging your source as a Docker image
- You're using a language without plugin SDK support

## How It Works

### Plugin Mode

**Python:** Source objects are called directly in-process via PyO3 — no subprocess, no serialization.

**Compiled languages:** Build a shared library (`.so`/`.dylib`/`.dll`) exporting the [C ABI](plugin.md#c-abi-reference). Bundlebase `dlopen`s it and uses the Arrow C Data Interface.

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

### Plugin Mode (Recommended for Python)

```python
import bundlebase.sync as bb
from my_source import MySource

# Python plugin — zero-copy, in-process
bundle = bb.create("my/data")
bundle.create_source_plugin(MySource())
results = bundle.fetch("base", "add")
```

### Plugin Mode (Shared Library)

```python
# Rust, Go, or Java — zero-copy via dlopen
bundle.create_source("plugin", {"call": "lib:./target/release/libmy_source.so"})
```

### IPC Mode (Subprocess)

=== "Async API"

    ```python
    import bundlebase as bb

    bundle = await (bb.create("my/data")
        .create_source("ipc", {"call": "python:my_source.py"}))

    results = await bundle.fetch("base", "add")
    ```

=== "Sync API"

    ```python
    import bundlebase.sync as bb

    bundle = (bb.create("my/data")
        .create_source("ipc", {"call": "python:my_source.py"}))

    results = bundle.fetch("base", "add")
    ```

=== "SQL"

    ```sql
    CREATE SOURCE ipc WITH (call = 'python:my_source.py')
    ```

## Call Syntax

The `call` argument specifies how to launch the subprocess:

| Syntax | Expands to | Example |
|--------|-----------|---------|
| `python:script.py` | `python script.py` | `python:sources/my_source.py` |
| `java:my.jar` | `java -jar my.jar` | `java:target/source.jar` |
| `docker:image` | `docker run -i --rm image` | `docker:myorg/my-source:latest` |
| `command args` | `command args` (whitespace split) | `./my-binary --config prod` |

## Docker Sources

Package any source as a Docker image:

```dockerfile
FROM python:3.13-slim
RUN pip install bundlebase-sdk pyarrow
COPY my_source.py /app/my_source.py
CMD ["python", "/app/my_source.py"]
```

Use with: `create_source("ipc", {"call": "docker:myorg/my-source:latest"})`

The container receives JSON-RPC on stdin and writes responses to stdout.

## SDK Quick Start

Each SDK handles the protocol for you. Implement the source interface and choose your entry point — `serve()` for IPC mode or the plugin export for zero-copy mode.

=== "Python"

    ```python
    from bundlebase_sdk import SourceFunction, Location, serve
    import pyarrow as pa

    class MySource(SourceFunction):
        def discover(self, attached_locations, **kwargs):
            return [Location("data.parquet", format="parquet", version="v1")]

        def data(self, location, **kwargs):
            return pa.table({"id": [1, 2, 3], "value": ["a", "b", "c"]})

    if __name__ == "__main__":
        serve(MySource())
    ```

    See the [Python SDK](python.md) reference for full API details.

=== "Go"

    ```go
    type MySource struct{}

    func (s *MySource) Discover(attached []string, args map[string]string) ([]sdk.Location, error) {
        return []sdk.Location{
            {Location: "data.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
        }, nil
    }

    func (s *MySource) Data(loc sdk.Location, args map[string]string) ([]arrow.Record, error) {
        // Build and return Arrow records
    }

    func main() { sdk.Serve(&MySource{}) }
    ```

    See the [Go SDK](go.md) reference for full API details.

=== "Java"

    ```java
    public class MySource implements SourceFunction {
        public List<Location> discover(List<String> attached, Map<String, String> args) {
            return List.of(new Location("data.parquet", true, "parquet", "v1"));
        }

        public VectorSchemaRoot data(Location loc, Map<String, String> args) {
            // Build and return Arrow VectorSchemaRoot
        }

        public static void main(String[] args) { Serve.run(new MySource()); }
    }
    ```

    See the [Java SDK](java.md) reference for full API details.

=== "Rust"

    ```rust
    struct MySource;

    impl SourceFunction for MySource {
        fn discover(&self, _attached: &[String], _args: &HashMap<String, String>)
            -> Result<Vec<Location>, Box<dyn std::error::Error>> {
            Ok(vec![Location { location: "data.parquet".into(), ..Location::new("data.parquet") }])
        }

        fn data(&self, _location: &Location, _args: &HashMap<String, String>)
            -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
            // Build and return Arrow RecordBatches
        }
    }

    fn main() { bundlebase_sdk::serve(&MySource); }
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
