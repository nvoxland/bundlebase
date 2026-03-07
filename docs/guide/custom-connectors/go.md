# Go SDK

Build custom Bundlebase connectors in Go.

## Installation

```bash
go get github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk
```

Requires `github.com/apache/arrow-go/v18` for Arrow types.

## Quick Start

```go
package main

import (
    "github.com/apache/arrow-go/v18/arrow"
    "github.com/apache/arrow-go/v18/arrow/array"
    "github.com/apache/arrow-go/v18/arrow/memory"
    sdk "github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk"
)

type ExampleConnector struct{}

func (s *ExampleConnector) Discover(attached []string, args map[string]string) ([]sdk.Location, error) {
    return []sdk.Location{
        {Location: "data.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
    }, nil
}

func (s *ExampleConnector) Data(loc sdk.Location, args map[string]string) ([]arrow.Record, error) {
    alloc := memory.NewGoAllocator()
    schema := arrow.NewSchema([]arrow.Field{
        {Name: "id", Type: arrow.PrimitiveTypes.Int64},
        {Name: "value", Type: arrow.BinaryTypes.String},
    }, nil)
    b := array.NewRecordBuilder(alloc, schema)
    defer b.Release()
    b.Field(0).(*array.Int64Builder).AppendValues([]int64{1, 2, 3}, nil)
    b.Field(1).(*array.StringBuilder).AppendValues([]string{"a", "b", "c"}, nil)
    return []arrow.Record{b.NewRecord()}, nil
}

func main() { sdk.Serve(&ExampleConnector{}) }
```

Build and use with:

```python
bundle.create_connector('example.connector', runner='ipc', logic='./example-connector')
bundle.create_source('example.connector')
```

## API Reference

### Connector Interface

```go
type Connector interface {
    Discover(attachedLocations []string, args map[string]string) ([]Location, error)
    Data(location Location, args map[string]string) ([]arrow.Record, error)
}
```

#### `Discover(attachedLocations, args)`

Return the available data locations.

| Parameter | Type | Description |
|-----------|------|-------------|
| `attachedLocations` | `[]string` | Locations already attached to the bundle |
| `args` | `map[string]string` | Extra arguments from the source configuration |

**Returns:** `([]Location, error)`

#### `Data(location, args)`

Return Arrow record batches for the given location. Return `nil` for no data.

| Parameter | Type | Description |
|-----------|------|-------------|
| `location` | `Location` | The location to fetch data for |
| `args` | `map[string]string` | Extra arguments from the source configuration |

**Returns:** `([]arrow.Record, error)`

### StableUrlProvider Interface

Optional interface for sources that provide stable URLs:

```go
type StableUrlProvider interface {
    StableUrl(location Location, args map[string]string) (*StableUrl, error)
}
```

Implement this interface on your connector struct to enable stable URL support. Return `nil` for no stable URL.

### Location

```go
type Location struct {
    Location string `json:"location"`
    MustCopy bool   `json:"must_copy"`
    Format   string `json:"format"`
    Version  string `json:"version"`
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `Location` | `string` | *(required)* | Identifier for this data file |
| `MustCopy` | `bool` | `false` (zero value) | Whether the data must be copied into the bundle |
| `Format` | `string` | `""` (zero value) | File format |
| `Version` | `string` | `""` (zero value) | Version string for change detection |

### StableUrl

```go
type StableUrl struct {
    URL string `json:"url"`
}
```

| Field | Type | Description |
|-------|------|-------------|
| `URL` | `string` | The stable URL string |

### Serve()

```go
func Serve(source Connector)
```

Run the connector as a JSON-RPC subprocess on stdin/stdout.

### ServeIO()

```go
func ServeIO(source Connector, r io.Reader, w io.Writer)
```

Run the connector on the given reader/writer. Used for testing.

## Complete Example

A source with multiple locations, multi-batch data, and extra argument handling:

```go
package main

import (
    "github.com/apache/arrow-go/v18/arrow"
    "github.com/apache/arrow-go/v18/arrow/array"
    "github.com/apache/arrow-go/v18/arrow/memory"
    sdk "github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk"
)

type DatabaseSource struct{}

func (s *DatabaseSource) Discover(attached []string, args map[string]string) ([]sdk.Location, error) {
    return []sdk.Location{
        {Location: "users.parquet", MustCopy: true, Format: "parquet", Version: "v2"},
        {Location: "orders.parquet", MustCopy: true, Format: "parquet", Version: "v1"},
    }, nil
}

func (s *DatabaseSource) Data(location sdk.Location, args map[string]string) ([]arrow.Record, error) {
    alloc := memory.NewGoAllocator()

    switch location.Location {
    case "users.parquet":
        schema := arrow.NewSchema([]arrow.Field{
            {Name: "id", Type: arrow.PrimitiveTypes.Int64},
            {Name: "name", Type: arrow.BinaryTypes.String},
        }, nil)

        // Return multiple batches for large datasets
        b1 := array.NewRecordBuilder(alloc, schema)
        defer b1.Release()
        b1.Field(0).(*array.Int64Builder).AppendValues([]int64{1, 2}, nil)
        b1.Field(1).(*array.StringBuilder).AppendValues([]string{"alice", "bob"}, nil)
        rec1 := b1.NewRecord()

        b2 := array.NewRecordBuilder(alloc, schema)
        defer b2.Release()
        b2.Field(0).(*array.Int64Builder).AppendValues([]int64{3}, nil)
        b2.Field(1).(*array.StringBuilder).AppendValues([]string{"charlie"}, nil)
        rec2 := b2.NewRecord()

        return []arrow.Record{rec1, rec2}, nil

    case "orders.parquet":
        schema := arrow.NewSchema([]arrow.Field{
            {Name: "order_id", Type: arrow.PrimitiveTypes.Int64},
            {Name: "user_id", Type: arrow.PrimitiveTypes.Int64},
            {Name: "amount", Type: arrow.PrimitiveTypes.Float64},
        }, nil)
        b := array.NewRecordBuilder(alloc, schema)
        defer b.Release()
        b.Field(0).(*array.Int64Builder).AppendValues([]int64{101, 102}, nil)
        b.Field(1).(*array.Int64Builder).AppendValues([]int64{1, 2}, nil)
        b.Field(2).(*array.Float64Builder).AppendValues([]float64{29.99, 49.99}, nil)
        return []arrow.Record{b.NewRecord()}, nil
    }

    return nil, nil
}

func main() { sdk.Serve(&DatabaseSource{}) }
```

## Testing

The SDK provides `ServeIO()` which accepts explicit reader/writer, letting you test without launching a subprocess:

```go
package main

import (
    "bytes"
    "encoding/binary"
    "encoding/json"
    "strings"
    "testing"

    sdk "github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk"
)

func makeRequest(method string, params map[string]interface{}, id int) string {
    req := map[string]interface{}{
        "jsonrpc": "2.0", "id": id, "method": method, "params": params,
    }
    b, _ := json.Marshal(req)
    return string(b) + "\n"
}

func readResponse(t *testing.T, data []byte, offset int) (map[string]interface{}, int) {
    t.Helper()
    end := bytes.IndexByte(data[offset:], '\n')
    line := data[offset : offset+end]
    var resp map[string]interface{}
    json.Unmarshal(line, &resp)
    return resp, offset + end + 1
}

func TestDiscover(t *testing.T) {
    input := makeRequest("discover", map[string]interface{}{"attached_locations": []string{}}, 1) +
        makeRequest("shutdown", nil, 2)

    var out bytes.Buffer
    sdk.ServeIO(&ExampleConnector{}, strings.NewReader(input), &out)

    resp, _ := readResponse(t, out.Bytes(), 0)
    result := resp["result"].(map[string]interface{})
    locations := result["locations"].([]interface{})
    if len(locations) != 1 {
        t.Fatalf("expected 1 location, got %d", len(locations))
    }
}

func TestDataReturnsArrow(t *testing.T) {
    input := makeRequest("data", map[string]interface{}{
        "location": map[string]interface{}{
            "location": "data.parquet", "must_copy": true, "format": "parquet", "version": "v1",
        },
    }, 1) + makeRequest("shutdown", nil, 2)

    var out bytes.Buffer
    sdk.ServeIO(&ExampleConnector{}, strings.NewReader(input), &out)

    data := out.Bytes()
    resp, offset := readResponse(t, data, 0)
    result := resp["result"].(map[string]interface{})
    if result["ok"] != true {
        t.Error("expected ok:true")
    }

    // Verify Arrow IPC frame is present
    length := binary.BigEndian.Uint32(data[offset : offset+4])
    if length == 0 {
        t.Error("expected non-zero Arrow IPC data")
    }
}
```

## Native Mode

Build your connector as a C-shared library for zero-copy in-process loading.

### Export the Source

Use `ExportConnector()` to register your connector, then build with `-buildmode=c-shared`:

```go
package main

import (
    "C"
    sdk "github.com/nvoxland/bundlebase/sdk/go/bundlebasesdk"
)

type ExampleConnector struct{}

// ... implement Discover() and Data() as usual ...

func init() {
    sdk.ExportConnector(&ExampleConnector{})
}

func main() {} // required for c-shared but never called
```

### Build and Use

```bash
go build -buildmode=c-shared -o example_connector.so .
```

```python
bundle.create_connector('example.connector', runner='lib', logic='./example_connector.so')
bundle.create_source('example.connector')
```

### How It Works

`ExportConnector()` stores your connector globally. The package exports `bundlebase_discover`, `bundlebase_data`, `bundlebase_free`, and `bundlebase_stable_url` via cgo. Data is transferred through the [Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html) using `arrow/cdata.ExportArrowRecordBatchReader`.

The same `Connector` interface works for both native and IPC — switch between them by changing only the entry point (`ExportConnector()` + `c-shared` build vs `func main() { Serve(&source) }`).

See [Native Mode](native.md) for the full C ABI reference.

## Error Handling

Errors returned from your `Discover()` or `Data()` methods are caught by the SDK and returned as JSON-RPC error responses with code `-32000`. The error message is included:

```go
func (s *ExampleConnector) Data(location sdk.Location, args map[string]string) ([]arrow.Record, error) {
    return nil, fmt.Errorf("database connection failed")
    // Bundlebase receives: {"error": {"code": -32000, "message": "database connection failed"}}
}
```

If a method not recognized by the protocol is called, the SDK returns a `-32601` (method not found) error automatically.

Bundlebase surfaces these errors to the user as connector failures during `FETCH`.
