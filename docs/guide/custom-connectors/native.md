# Native Mode

Native mode loads your connector in-process for zero-copy Arrow data transfer, eliminating the subprocess and serialization overhead of [IPC mode](index.md).

## When to Use Native vs IPC

| | **Native** | **IPC** |
|---|---|---|
| **Performance** | Zero-copy Arrow (fastest) | Serialized Arrow IPC over pipes |
| **Isolation** | Runs in-process | Separate subprocess |
| **Languages** | Python (in-process), Rust/Go/Java (shared library) | Any language with stdin/stdout |
| **Setup** | Python: direct object; compiled: build `.so` | Script or binary |
| **Best for** | Performance-critical pipelines, large datasets | Polyglot environments, simple scripts, Docker |

**Use native when:** You need maximum throughput and your connector is written in Python, Rust, Go, or Java.

**Use IPC when:** You want process isolation, use Docker, or work in a language without an SDK.

!!! warning "Required Configuration"
    Native connectors require the `allow_external_code` configuration setting. See [Configuration](index.md#configuration) for details.

## How It Works

### Python (PyO3 In-Process)

Python connectors run directly inside the Bundlebase process via PyO3. Arrow data is transferred through shared memory; no serialization.

```python
import bundlebase.sync as bb

bundle = bb.create("my/data")
bundle.import_temp_connector('example.connector', 'python::example_connector:ExampleConnector')
bundle.create_source('example.connector')
bundle.fetch("base", "add")
```

The `Connector` class is identical whether you use native or IPC mode. The only difference is how you create the connector: `runtime='python'` with a `module:Class` value instead of `runtime='ipc'` with a command. Python connectors use `IMPORT TEMP CONNECTOR` since Python code is runtime-only and cannot be bundled.

### Shared Libraries (Rust, Go, Java)

Compiled languages build a shared library (`.so` / `.dylib` / `.dll`) that exports the [C ABI](#c-abi-reference). Bundlebase `dlopen`s it and uses the [Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html) for zero-copy streaming.

```python
# Load a Rust, Go, or Java shared library
bundle.import_connector('example.connector', 'ffi::./target/release/libexample_connector.so')
bundle.create_source('example.connector')
```

Each language has its own approach to generating the C ABI:

- **Rust** -- `export_source!` macro generates `extern "C"` functions
- **Go** -- cgo `//export` directives
- **Java** -- Project Panama (Java 22+): a thin C bootstrap starts the JVM once, then all ABI calls route through Panama upcall stubs for minimal overhead

## Runtime Values for Native Mode

The `runtime` parameter determines the native loading strategy:

| Type | Strategy | Used by |
|------|----------|---------|
| `python` | PyO3 in-process (use with `IMPORT TEMP CONNECTOR`) | Python |
| `ffi`    | `dlopen` + Arrow C Data Interface (use with `IMPORT CONNECTOR`) | Rust, Go, Java |

## C ABI Reference

Shared libraries must export these symbols:

### Required

```c
// Discover available data locations
// args_json: JSON with source args + "attached_locations" array
// out_json: Caller-allocated pointer; set to malloc'd JSON string
// Returns: 0 on success, non-zero on error (out_json may contain error message)
int32_t bundlebase_discover(const char* args_json, char** out_json);

// Provide data for a location
// location_json: JSON with location fields (location, must_copy, format, version)
// args_json: JSON with source args (excluding call/copy)
// out: Caller-allocated ArrowArrayStream; populate via Arrow C Data Interface
// Returns: 0 on success, non-zero on error
int32_t bundlebase_data(const char* location_json, const char* args_json,
                        struct ArrowArrayStream* out);

// Free a string allocated by discover or stable_url
void bundlebase_free(char* ptr);
```

### Optional

```c
// Provide a stable URL for caching
// Returns: 0 on success, out_json contains {"url": "..."} or is left null
int32_t bundlebase_stable_url(const char* location_json, const char* args_json,
                              char** out_json);
```

### JSON Schemas

**discover args_json:**
```json
{
  "attached_locations": ["loc1", "loc2"],
  "custom_arg": "value"
}
```

**discover response (out_json):**
```json
{
  "locations": [
    {"location": "file.parquet", "must_copy": true, "format": "parquet", "version": "v1", "num_rows": 1234}
  ]
}
```

`num_rows` is **required** on every location. Set it to a non-negative integer when the connector can determine the row count cheaply (Parquet readers can read it from the footer; sources with a manifest can look it up), or to JSON `null` when counting would require fully parsing the data. `null` is preserved through to `FETCH ... DRY RUN`'s `rows_after` column so users can tell "0 rows" from "I don't know yet". A missing `num_rows` key is treated as a connector bug and rejected, so declare it explicitly even when unknown.

**data location_json:**
```json
{"location": "file.parquet", "must_copy": true, "format": "parquet", "version": "v1", "num_rows": 1234}
```

**data args_json:**
```json
{"custom_arg": "value"}
```

## Language Guides

Each SDK provides helpers that generate the C ABI functions for you:

- **[Python](python.md#native-mode)** -- `IMPORT TEMP CONNECTOR` with `runtime='python'`, `entrypoint='module:Class'` (no shared library needed)
- **[Rust](rust.md#native-mode)** -- `export_source!(ExampleConnector::new())` (use `runtime='ffi'`)
- **[Go](go.md#native-mode)** -- `ExportConnector(&ExampleConnector{})` (use `runtime='ffi'`)
- **[Java](java.md#native-mode)** -- `PluginExport.register(new ExampleConnector())` (use `runtime='ffi'`)

## Connector Arguments

These are passed to `IMPORT CONNECTOR` or `IMPORT TEMP CONNECTOR`:

| Argument | Required | Description |
|----------|----------|-------------|
| `runtime` | Yes | `'python'` or `'ffi'` |
| `entrypoint` | Yes | Source to load: `module:Class` (for `python`) or path to shared library (for `ffi`) |
| `platform` | No | Target platform (e.g., `linux/amd64`, `darwin/arm64`, `*/*` default) |

For `runtime='python'`, use `IMPORT TEMP CONNECTOR` (runtime-only). For `runtime='ffi'`, use `IMPORT CONNECTOR` (persisted into the bundle).

Extra arguments passed to `CREATE SOURCE` are forwarded to the connector's `discover()` and `data()` methods, just like IPC mode.

## Multi-platform Connectors

`IMPORT CONNECTOR` registers one binary per `(name, platform)` pair. To ship a fat connector that runs on multiple OS/arch combinations, register all binaries up front. At fetch time the entry whose platform matches the host wins.

Two SQL forms cover the common cases:

**Explicit map** -- list every platform you want to support:

```sql
IMPORT CONNECTOR acme.weather FROM {
    'linux/amd64'   : 'ffi::./weather-linux-amd64.so',
    'linux/arm64'   : 'ffi::./weather-linux-arm64.so',
    'darwin/arm64'  : 'ffi::./weather-darwin-arm64.dylib',
    'windows/amd64' : 'ffi::./weather-windows-amd64.dll'
};
```

**Glob form** -- let bundlebase scan a directory for matching files:

```sql
IMPORT CONNECTOR acme.weather FROM 'ffi::./weather-{os}-{arch}.{ext}';
```

Placeholders: `{os}` (linux/darwin/windows), `{arch}` (amd64/arm64/...), `{ext}` (so/dylib/dll, validated against `{os}` if both appear).

Each binary is copied into the bundle's content-addressed data directory and verified: fully via `dlopen` for the binary that matches your host, and via shared-library header inspection (ELF / Mach-O / PE magic + arch byte) for foreign-platform binaries the build host can't load. If no entry covers your host, the import succeeds with a warning so you can still build a bundle that targets only deployment hosts.

## Bundling Connector Source

Optionally ship the connector's source code with the bundle. Use `WITH (src = '/path/to.zip')`:

```sql
IMPORT CONNECTOR acme.weather FROM 'ffi::./lib.so'
    WITH (platform = 'linux/amd64', src = './weather-source.zip');
```

The archive is copied into the bundle's data directory (content-addressed), shipped with empty exports, and extractable later via:

```sql
EXPORT SOURCE acme.weather TO '/tmp/weather-source.zip';
```

When combined with the multi-platform forms, all entries share the same source archive: bundle once, ship for every platform.
