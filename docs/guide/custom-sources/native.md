# Native Source Mode

Native sources load your source function in-process for zero-copy Arrow data transfer, eliminating the subprocess and serialization overhead of [IPC mode](index.md).

## When to Use Native vs IPC

| | **Native** | **IPC** |
|---|---|---|
| **Performance** | Zero-copy Arrow (fastest) | Serialized Arrow IPC over pipes |
| **Isolation** | Runs in-process | Separate subprocess |
| **Languages** | Python (in-process), Rust/Go/Java (shared library) | Any language with stdin/stdout |
| **Setup** | Python: direct object; compiled: build `.so` | Script or binary |
| **Best for** | Performance-critical pipelines, large datasets | Polyglot environments, simple scripts, Docker |

**Use native when:** You need maximum throughput and your source is in Python, Rust, Go, or Java.

**Use IPC when:** You want process isolation, use Docker, or work in a language without an SDK.

!!! warning "Required Configuration"
    Native sources require the `allow_external_code` configuration setting. See [Configuration](index.md#configuration) for details.

## How It Works

### Python (PyO3 In-Process)

Python sources run directly inside the Bundlebase process via PyO3. Arrow data is transferred through shared memory — no serialization.

```python
import bundlebase.sync as bb

bundle = bb.create("my/data")
bundle.create_connector('example.connector')
bundle.set_temporary_connector_logic('example.connector', type_='python', logic='example_connector:ExampleConnector')
bundle.create_source('example.connector')
bundle.fetch("base", "add")
```

The `SourceFunction` class is identical whether you use native or IPC mode. The only difference is how you configure the source logic: `type_='python'` with a `module:Class` logic value instead of `type_='ipc'` with a command. Python sources use `set_temporary_connector_logic()` since Python code is runtime-only and cannot be bundled.

### Shared Libraries (Rust, Go, Java)

Compiled languages build a shared library (`.so` / `.dylib` / `.dll`) that exports the [C ABI](#c-abi-reference). Bundlebase `dlopen`s it and uses the [Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html) for zero-copy streaming.

```python
# Load a Rust, Go, or Java shared library
bundle.create_connector('example.connector')
bundle.set_connector_logic('example.connector', type_='lib', logic='./target/release/libexample_connector.so')
bundle.create_source('example.connector')
```

Each language has its own approach to generating the C ABI:

- **Rust** — `export_source!` macro generates `extern "C"` functions
- **Go** — cgo `//export` directives
- **Java** — Project Panama (Java 22+): a thin C bootstrap starts the JVM once, then all ABI calls route through Panama upcall stubs for minimal overhead

## Type Values for Native Mode

The `type_` parameter determines the native loading strategy:

| Type | Strategy | Used by |
|------|----------|---------|
| `python` | PyO3 in-process (use with `set_temporary_connector_logic()`) | Python |
| `lib` | `dlopen` + Arrow C Data Interface (use with `set_connector_logic()`) | Rust, Go, Java |

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
    {"location": "file.parquet", "must_copy": true, "format": "parquet", "version": "v1"}
  ]
}
```

**data location_json:**
```json
{"location": "file.parquet", "must_copy": true, "format": "parquet", "version": "v1"}
```

**data args_json:**
```json
{"custom_arg": "value"}
```

## Language Guides

Each SDK provides helpers that generate the C ABI functions for you:

- **[Python](python.md#native-mode)** — `set_temporary_connector_logic(..., type_='python', logic='module:Class')` (no shared library needed)
- **[Rust](rust.md#native-mode)** — `export_source!(ExampleConnector::new())`
- **[Go](go.md#native-mode)** — `ExportSource(&ExampleConnector{})`
- **[Java](java.md#native-mode)** — `PluginExport.register(new ExampleConnector())`

## Source Logic Arguments

These are passed to `set_connector_logic()` or `set_temporary_connector_logic()`:

| Argument | Required | Description |
|----------|----------|-------------|
| `type_` | Yes | `'python'` or `'lib'` |
| `logic` | Yes | Source to load: `module:Class` (for `python`) or path to shared library (for `lib`) |
| `platform` | No | Target platform (e.g., `linux/amd64`, `darwin/arm64`, `*/*` default) |

For `type_='python'`, use `set_temporary_connector_logic()` (runtime-only). For `type_='lib'`, use `set_connector_logic()` (persisted into the bundle).

Extra arguments passed to `create_source()` are forwarded to the source's `discover()` and `data()` methods, just like IPC mode.
