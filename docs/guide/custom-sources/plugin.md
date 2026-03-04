# Plugin Source Mode

Plugin sources load your source function in-process for zero-copy Arrow data transfer, eliminating the subprocess and serialization overhead of [IPC mode](index.md).

## When to Use Plugin vs IPC

| | **Plugin** | **IPC** |
|---|---|---|
| **Performance** | Zero-copy Arrow (fastest) | Serialized Arrow IPC over pipes |
| **Isolation** | Runs in-process | Separate subprocess |
| **Languages** | Python (in-process), Rust/Go/Java (shared library) | Any language with stdin/stdout |
| **Setup** | Python: direct object; compiled: build `.so` | Script or binary |
| **Best for** | Performance-critical pipelines, large datasets | Polyglot environments, simple scripts, Docker |

**Use plugin when:** You need maximum throughput and your source is in Python, Rust, Go, or Java.

**Use IPC when:** You want process isolation, use Docker, or work in a language without an SDK.

## How It Works

### Python (PyO3 In-Process)

Python sources run directly inside the Bundlebase process via PyO3. Arrow data is transferred through shared memory — no serialization.

```python
import bundlebase.sync as bb
from my_source import MySource

bundle = bb.create("my/data")
bundle.create_source_plugin(MySource())
bundle.fetch(mode="add")
```

The `SourceFunction` class is identical whether you use plugin or IPC mode. The only difference is the entry point: `create_source_plugin(obj)` instead of `create_source("ipc", {"call": "python:script.py"})`.

### Shared Libraries (Rust, Go, Java)

Compiled languages build a shared library (`.so` / `.dylib` / `.dll`) that exports the [C ABI](#c-abi-reference). Bundlebase `dlopen`s it and uses the [Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html) for zero-copy streaming.

```python
# Load a Rust, Go, or Java shared library
bundle.create_source("plugin", {"call": "lib:./target/release/libmy_source.so"})
```

Each language has its own approach to generating the C ABI:

- **Rust** — `export_source!` macro generates `extern "C"` functions
- **Go** — cgo `//export` directives
- **Java** — Project Panama (Java 22+): a thin C bootstrap starts the JVM once, then all ABI calls route through Panama upcall stubs for minimal overhead

## Call Syntax

The `call` argument determines the loading strategy:

| Syntax | Strategy | Used by |
|--------|----------|---------|
| `python:module:Class` | PyO3 in-process | Python (`create_source_plugin` handles this automatically) |
| `lib:/path/to/lib.so` | `dlopen` + Arrow C Data Interface | Rust, Go, Java |

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

- **[Python](python.md#plugin-mode)** — `create_source_plugin(MySource())` (no shared library needed)
- **[Rust](rust.md#plugin-mode)** — `export_source!(MySource::new())`
- **[Go](go.md#plugin-mode)** — `ExportSource(&MySource{})`
- **[Java](java.md#plugin-mode)** — `PluginExport.register(new MySource())`

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `call` | Yes | Source to load: `lib:/path/to/lib.so` or `python:module:Class` |
| `copy` | No | `"true"` to copy data into bundle (default), `"false"` to reference in place |

Extra arguments are forwarded to the source's `discover()` and `data()` methods, just like IPC mode.
