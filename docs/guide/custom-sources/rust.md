# Rust SDK

Build custom Bundlebase source functions in Rust.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
bundlebase-sdk = "0.7"
arrow = "57"
```

## Quick Start

```rust
use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use bundlebase_sdk::{Location, SourceFunction};
use std::collections::HashMap;
use std::sync::Arc;

struct ExampleConnector;

impl SourceFunction for ExampleConnector {
    fn discover(
        &self, _attached: &[String], _args: &HashMap<String, String>,
    ) -> Result<Vec<Location>, Box<dyn std::error::Error>> {
        Ok(vec![Location {
            location: "data.parquet".into(),
            must_copy: true,
            format: "parquet".into(),
            version: "v1".into(),
        }])
    }

    fn data(
        &self, _location: &Location, _args: &HashMap<String, String>,
    ) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("value", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(schema, vec![
            Arc::new(Int64Array::from(vec![1, 2, 3])),
            Arc::new(StringArray::from(vec!["a", "b", "c"])),
        ])?;
        Ok(Some(vec![batch]))
    }
}

fn main() { bundlebase_sdk::serve(&ExampleConnector); }
```

Build and use with:

```python
bundle.create_connector('example.connector')
bundle.set_connector_logic('example.connector', type_='ipc', logic='./target/release/example-connector')
bundle.create_source('example.connector')
```

## API Reference

### SourceFunction Trait

```rust
pub trait SourceFunction {
    fn discover(
        &self, attached_locations: &[String], args: &HashMap<String, String>,
    ) -> Result<Vec<Location>, Box<dyn std::error::Error>>;

    fn data(
        &self, location: &Location, args: &HashMap<String, String>,
    ) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>>;

    fn stable_url(
        &self, _location: &Location, _args: &HashMap<String, String>,
    ) -> Result<Option<StableUrl>, Box<dyn std::error::Error>> {
        Ok(None)
    }
}
```

#### `discover(&self, attached_locations, args)`

Return the available data locations.

| Parameter | Type | Description |
|-----------|------|-------------|
| `attached_locations` | `&[String]` | Locations already attached to the bundle |
| `args` | `&HashMap<String, String>` | Extra arguments from the source configuration |

**Returns:** `Result<Vec<Location>, Box<dyn std::error::Error>>`

#### `data(&self, location, args)`

Return Arrow record batches for the given location. Return `Ok(None)` for no data.

| Parameter | Type | Description |
|-----------|------|-------------|
| `location` | `&Location` | The location to fetch data for |
| `args` | `&HashMap<String, String>` | Extra arguments from the source configuration |

**Returns:** `Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>>`

#### `stable_url(&self, location, args)`

Return a stable URL for the given location. Default returns `Ok(None)`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `location` | `&Location` | The location to get a URL for |
| `args` | `&HashMap<String, String>` | Extra arguments from the source configuration |

**Returns:** `Result<Option<StableUrl>, Box<dyn std::error::Error>>`

### Location

```rust
pub struct Location {
    pub location: String,
    pub must_copy: bool,    // default: true
    pub format: String,     // default: "parquet"
    pub version: String,    // default: ""
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `location` | `String` | *(required)* | Identifier for this data file |
| `must_copy` | `bool` | `true` | Whether the data must be copied into the bundle |
| `format` | `String` | `"parquet"` | File format |
| `version` | `String` | `""` | Version string for change detection |

Convenience constructor: `Location::new("path")` sets defaults for all other fields.

### StableUrl

```rust
pub struct StableUrl {
    pub url: String,
}
```

| Field | Type | Description |
|-------|------|-------------|
| `url` | `String` | The stable URL string |

### serve()

```rust
pub fn serve(source: &dyn SourceFunction)
```

Run the source function as a JSON-RPC subprocess on stdin/stdout.

### serve_io()

```rust
pub fn serve_io(source: &dyn SourceFunction, r: &mut dyn BufRead, w: &mut dyn Write)
```

Run the source function on the given reader/writer. Used for testing.

## Complete Example

A source with multiple locations, multi-batch data, stable URLs, and extra argument handling:

```rust
use arrow::array::{Int64Array, StringArray, Float64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use bundlebase_sdk::{Location, SourceFunction, StableUrl};
use std::collections::HashMap;
use std::sync::Arc;

struct DatabaseSource;

impl SourceFunction for DatabaseSource {
    fn discover(
        &self, _attached: &[String], _args: &HashMap<String, String>,
    ) -> Result<Vec<Location>, Box<dyn std::error::Error>> {
        Ok(vec![
            Location {
                location: "users.parquet".into(),
                must_copy: true,
                format: "parquet".into(),
                version: "v2".into(),
            },
            Location {
                location: "orders.parquet".into(),
                must_copy: true,
                format: "parquet".into(),
                version: "v1".into(),
            },
        ])
    }

    fn data(
        &self, location: &Location, _args: &HashMap<String, String>,
    ) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
        match location.location.as_str() {
            "users.parquet" => {
                let schema = Arc::new(Schema::new(vec![
                    Field::new("id", DataType::Int64, false),
                    Field::new("name", DataType::Utf8, false),
                ]));
                // Return multiple batches for large datasets
                let batch1 = RecordBatch::try_new(schema.clone(), vec![
                    Arc::new(Int64Array::from(vec![1, 2])),
                    Arc::new(StringArray::from(vec!["alice", "bob"])),
                ])?;
                let batch2 = RecordBatch::try_new(schema, vec![
                    Arc::new(Int64Array::from(vec![3])),
                    Arc::new(StringArray::from(vec!["charlie"])),
                ])?;
                Ok(Some(vec![batch1, batch2]))
            }
            "orders.parquet" => {
                let schema = Arc::new(Schema::new(vec![
                    Field::new("order_id", DataType::Int64, false),
                    Field::new("user_id", DataType::Int64, false),
                    Field::new("amount", DataType::Float64, false),
                ]));
                let batch = RecordBatch::try_new(schema, vec![
                    Arc::new(Int64Array::from(vec![101, 102])),
                    Arc::new(Int64Array::from(vec![1, 2])),
                    Arc::new(Float64Array::from(vec![29.99, 49.99])),
                ])?;
                Ok(Some(vec![batch]))
            }
            _ => Ok(None),
        }
    }

    fn stable_url(
        &self, location: &Location, _args: &HashMap<String, String>,
    ) -> Result<Option<StableUrl>, Box<dyn std::error::Error>> {
        match location.location.as_str() {
            "users.parquet" => Ok(Some(StableUrl {
                url: "https://db.example.com/exports/users-v2.parquet".into(),
            })),
            _ => Ok(None),
        }
    }
}

fn main() { bundlebase_sdk::serve(&DatabaseSource); }
```

## Testing

The SDK provides `serve_io()` which accepts explicit reader/writer, letting you test without launching a subprocess:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn make_request(method: &str, params: serde_json::Value, id: u64) -> String {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        format!("{}\n", serde_json::to_string(&req).unwrap())
    }

    fn read_response(data: &[u8], offset: usize) -> (serde_json::Value, usize) {
        let remaining = &data[offset..];
        let end = remaining.iter().position(|&b| b == b'\n').unwrap();
        let line = &remaining[..end];
        let resp: serde_json::Value = serde_json::from_slice(line).unwrap();
        (resp, offset + end + 1)
    }

    #[test]
    fn test_discover() {
        let input = make_request(
            "discover",
            serde_json::json!({"attached_locations": []}),
            1,
        ) + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        bundlebase_sdk::serve_io(
            &ExampleConnector,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (resp, _) = read_response(&output, 0);
        let locations = resp["result"]["locations"].as_array().unwrap();
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0]["location"], "data.parquet");
    }

    #[test]
    fn test_data_returns_arrow() {
        let input = make_request(
            "data",
            serde_json::json!({
                "location": {"location": "data.parquet", "must_copy": true,
                              "format": "parquet", "version": "v1"}
            }),
            1,
        ) + &make_request("shutdown", serde_json::json!({}), 2);

        let mut output = Vec::new();
        bundlebase_sdk::serve_io(
            &ExampleConnector,
            &mut Cursor::new(input.as_bytes()),
            &mut output,
        );

        let (resp, offset) = read_response(&output, 0);
        assert_eq!(resp["result"]["ok"], true);

        // Verify Arrow IPC frame
        let length = u32::from_be_bytes(
            output[offset..offset + 4].try_into().unwrap()
        ) as usize;
        assert!(length > 0, "Expected non-zero Arrow IPC data");
    }
}
```

## Native Mode

Build your source as a shared library for zero-copy in-process loading.

### Setup

Add `crate-type = ["cdylib"]` to your `Cargo.toml`:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
bundlebase-sdk = "0.7"
arrow = "57"
serde_json = "1"
```

### Export the Source

Use the `export_source!` macro to generate the C ABI entry points:

```rust
use bundlebase_sdk::{SourceFunction, Location, export_source};
use arrow::record_batch::RecordBatch;
use std::collections::HashMap;

struct ExampleConnector;

impl SourceFunction for ExampleConnector {
    fn discover(&self, _attached: &[String], _args: &HashMap<String, String>)
        -> Result<Vec<Location>, Box<dyn std::error::Error>> {
        Ok(vec![Location::new("data.parquet")])
    }

    fn data(&self, _location: &Location, _args: &HashMap<String, String>)
        -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
        // ... build and return Arrow data
        Ok(None)
    }
}

// Generates bundlebase_discover, bundlebase_data, bundlebase_free, bundlebase_stable_url
export_source!(ExampleConnector);
```

### Build and Use

```bash
cargo build --release
```

```python
bundle.create_connector('example.connector')
bundle.set_connector_logic('example.connector', type_='lib', logic='target/release/libexample_connector.so')
bundle.create_source('example.connector')
```

### How It Works

The `export_source!` macro:

1. Creates a `OnceLock`-backed singleton of your source
2. Generates `extern "C"` functions matching the [Bundlebase C ABI](native.md#c-abi-reference)
3. Uses Arrow's `FFI_ArrowArrayStream` for zero-copy data export

The same `SourceFunction` trait works for both native and IPC — switch between them by changing only the entry point (`export_source!` vs `fn main() { serve(&source) }`).

See [Native Source Mode](native.md) for the full C ABI reference.

## Error Handling

Errors returned from your `discover()`, `data()`, or `stable_url()` methods are caught by the SDK and returned as JSON-RPC error responses with code `-32000`. The error message is included:

```rust
fn data(
    &self, _location: &Location, _args: &HashMap<String, String>,
) -> Result<Option<Vec<RecordBatch>>, Box<dyn std::error::Error>> {
    Err("Database connection failed".into())
    // Bundlebase receives: {"error": {"code": -32000, "message": "Database connection failed"}}
}
```

If a method not recognized by the protocol is called, the SDK returns a `-32601` (method not found) error automatically.

Bundlebase surfaces these errors to the user as source function failures during `fetch()`.
